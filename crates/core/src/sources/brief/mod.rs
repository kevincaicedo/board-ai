//! Research briefs (D-185): what a coding harness writes after reading a code source, in one
//! fixed Markdown shape, parsed here into a typed, bounded [`Brief`].
//!
//! ```text
//! # Brief: <topic>
//! ## Overview
//! ## Components        - <id> · <name> · <kind> · <path> — <role>
//! ## Relations         - <id> → <id>: <label>
//! ## Flows             ### <name>  1. <step> (<path>:<line>)
//! ## Diagram           <diagram title="…"> …narration tags with ids… </diagram>
//! ## Details           ### <component id>  notes, and optionally a nested <diagram>
//! ## Citations         - <path>:<line>[-<line>]
//! ```
//!
//! The brief is harness output: untrusted data that quotes repo text (D-107). The parser keeps
//! what has the shape and drops and counts the rest, so a malformed answer still gives what it
//! got right. Every cited line is read natively before a chip may show it (law 6, D-123). The
//! diagram stays raw, bounded text with the component ids it names: staged layout (T-032)
//! plugs in there, and until then the talker gets it as a reference.

mod parse;
mod request;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::ids::ElementId;
use crate::limits::{
    BRIEF_CITATION_LINES_MAX, BRIEF_CITATION_PATH_MAX_BYTES, BRIEF_CITATIONS_MAX,
    SPOKEN_SOURCE_NAME_MAX_BYTES,
};
use crate::narration::tags::bounded;

pub use parse::parse;
pub use request::request_prompt;

/// What a brief is about: the whole source, or one part of it in more detail ("more detail on
/// the arena", D-188).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "topic", rename_all = "snake_case")]
pub enum BriefTopic {
    Overview,
    Component { name: String },
}

impl BriefTopic {
    /// A component topic, its name held to the bound of a spoken name.
    #[must_use]
    pub fn component(name: &str) -> Self {
        Self::Component {
            name: bounded(name, SPOKEN_SOURCE_NAME_MAX_BYTES),
        }
    }

    /// How the cache names it: `overview`, or `component-<letters and digits>`.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Overview => "overview".to_owned(),
            Self::Component { name } => format!("component-{}", super::name_key(name)),
        }
    }

    /// How BoardAI says it ("the overview", "the arena").
    #[must_use]
    pub fn spoken(&self) -> String {
        match self {
            Self::Overview => "the overview".to_owned(),
            Self::Component { name } => format!("the {name}"),
        }
    }
}

/// A cited line range, `path:first[-last]`, 1-based and inclusive as people write them. The path
/// is relative to the source's folder, never absolute and never with `..`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Citation {
    pub path: String,
    pub first_line: u32,
    pub last_line: u32,
}

impl Citation {
    /// Parses `path:42` or `path:42-58` (also `L42`). A range wider than
    /// [`BRIEF_CITATION_LINES_MAX`] keeps its first lines. The path is relative to the source's
    /// folder; it may start with `../` (a file elsewhere in the source's repository, D-226),
    /// never climb back in the middle (`a/../b`), and the runtime reads it before it is kept.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_matches('`');
        let (path, lines) = text.rsplit_once(':')?;
        let lines = lines.trim_start_matches(['L', 'l']);
        let (first, last) = match lines.split_once('-') {
            Some((first, last)) => (first, last.trim_start_matches(['L', 'l'])),
            None => (lines, lines),
        };
        let first: u32 = first.parse().ok()?;
        let last: u32 = last.parse().ok()?;
        let safe_path = !path.is_empty()
            && path.len() <= BRIEF_CITATION_PATH_MAX_BYTES
            && !path.starts_with('/')
            && !path.contains('\\')
            && !path.chars().any(char::is_whitespace)
            && climbs_only_first(path);
        if !safe_path || first == 0 || last < first {
            return None;
        }
        let path = path.strip_prefix("./").unwrap_or(path).to_owned();
        Some(Self {
            path,
            first_line: first,
            last_line: last.min(first.saturating_add(BRIEF_CITATION_LINES_MAX - 1)),
        })
    }

    /// `path:42` or `path:42-58`.
    #[must_use]
    pub fn label(&self) -> String {
        if self.first_line == self.last_line {
            format!("{}:{}", self.path, self.first_line)
        } else {
            format!("{}:{}-{}", self.path, self.first_line, self.last_line)
        }
    }
}

/// `../../crates/x.rs`: `..` only before the first name, and at least one name.
fn climbs_only_first(path: &str) -> bool {
    let mut named = false;
    for part in path.split('/') {
        match part {
            "" => return false,
            ".." if named => return false,
            ".." => {}
            _ => named = true,
        }
    }
    named
}

/// One part of the code the brief names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    pub id: ElementId,
    pub name: String,
    /// crate, module, type, service…, as the harness wrote it; may be empty.
    pub kind: String,
    /// Relative to the source's folder; may be empty.
    pub path: String,
    pub role: String,
}

/// `from → to: label`, between two listed components.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub from: ElementId,
    pub to: ElementId,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flow {
    pub name: String,
    pub steps: Vec<FlowStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowStep {
    pub text: String,
    pub citation: Option<Citation>,
}

/// A `<diagram>` block as the harness wrote it, in BoardAI's narration vocabulary: the raw text,
/// bounded, and the component ids its tags name. Staged layout (T-032) lays it out; until then
/// the talker gets the text as a reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BriefDiagram {
    pub title: String,
    pub text: String,
    /// Component ids named by an `id`, `from`, `to` or `in` attribute, in order of appearance.
    pub ids: Vec<ElementId>,
}

/// More about one component, for drilling down (D-188).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    pub component: ElementId,
    pub notes: String,
    pub diagram: Option<BriefDiagram>,
}

/// What the parser left out, by kind, so the talker and the log can say so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BriefDropped {
    pub components: u32,
    pub relations: u32,
    pub flows: u32,
    pub steps: u32,
    pub details: u32,
    pub citations: u32,
    /// Lines that fit no section's shape, or were over the line bound.
    pub lines: u32,
    /// Bytes past `BRIEF_MAX_BYTES` and past a section's own bound.
    pub truncated: bool,
}

impl BriefDropped {
    #[must_use]
    pub fn total(&self) -> u32 {
        self.components
            + self.relations
            + self.flows
            + self.steps
            + self.details
            + self.citations
            + self.lines
            + u32::from(self.truncated)
    }
}

/// A parsed, bounded research brief.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    pub topic: String,
    pub overview: String,
    pub components: Vec<Component>,
    pub relations: Vec<Relation>,
    pub flows: Vec<Flow>,
    pub diagram: Option<BriefDiagram>,
    pub details: Vec<Detail>,
    pub citations: Vec<Citation>,
    pub dropped: BriefDropped,
}

impl Brief {
    /// Something to explain from: an overview or at least one component.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.overview.is_empty() || !self.components.is_empty()
    }

    /// The component a spoken name means: its id or its name, compared by letters and digits.
    #[must_use]
    pub fn component_named(&self, name: &str) -> Option<&Component> {
        let key = super::name_key(name);
        if key.is_empty() {
            return None;
        }
        self.components.iter().find(|component| {
            super::name_key(component.id.as_str()) == key || super::name_key(&component.name) == key
        })
    }

    /// Every citation the brief relies on (its section, then its flows' steps), each once, at
    /// most [`BRIEF_CITATIONS_MAX`].
    #[must_use]
    pub fn all_citations(&self) -> Vec<Citation> {
        let mut seen = BTreeSet::new();
        let steps = self
            .flows
            .iter()
            .flat_map(|flow| &flow.steps)
            .filter_map(|step| step.citation.as_ref());
        self.citations
            .iter()
            .chain(steps)
            .filter(|citation| seen.insert((*citation).clone()))
            .take(BRIEF_CITATIONS_MAX)
            .cloned()
            .collect()
    }

    /// Keeps only the citations that were read natively; a flow step keeps its words without
    /// an unread citation. Returns how many citations were removed.
    pub fn keep_citations(&mut self, read: &BTreeSet<Citation>) -> usize {
        let before = self.citations.len();
        self.citations.retain(|citation| read.contains(citation));
        let mut removed = before - self.citations.len();
        for step in self.flows.iter_mut().flat_map(|flow| &mut flow.steps) {
            if step
                .citation
                .as_ref()
                .is_some_and(|citation| !read.contains(citation))
            {
                step.citation = None;
                removed += 1;
            }
        }
        removed
    }

    /// The brief in its canonical, bounded Markdown: what the cache keeps and the talker reads.
    /// Parsing it again gives the same brief, with nothing dropped.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# Brief: {}\n", self.topic);
        if !self.overview.is_empty() {
            let _ = write!(out, "\n## Overview\n{}\n", self.overview);
        }
        if !self.components.is_empty() {
            out.push_str("\n## Components\n");
            for part in &self.components {
                let _ = writeln!(
                    out,
                    "- {} · {} · {} · {} — {}",
                    part.id.as_str(),
                    part.name,
                    part.kind,
                    part.path,
                    part.role
                );
            }
        }
        if !self.relations.is_empty() {
            out.push_str("\n## Relations\n");
            for relation in &self.relations {
                let _ = writeln!(
                    out,
                    "- {} → {}: {}",
                    relation.from.as_str(),
                    relation.to.as_str(),
                    relation.label
                );
            }
        }
        self.render_flows(&mut out);
        if let Some(diagram) = &self.diagram {
            let _ = write!(out, "\n## Diagram\n{}\n", diagram.text);
        }
        self.render_details(&mut out);
        if !self.citations.is_empty() {
            out.push_str("\n## Citations\n");
            for citation in &self.citations {
                let _ = writeln!(out, "- {}", citation.label());
            }
        }
        out
    }

    fn render_flows(&self, out: &mut String) {
        if self.flows.is_empty() {
            return;
        }
        out.push_str("\n## Flows\n");
        for flow in &self.flows {
            let _ = writeln!(out, "### {}", flow.name);
            for (number, step) in (1..).zip(&flow.steps) {
                match &step.citation {
                    Some(citation) => {
                        let _ = writeln!(out, "{number}. {} ({})", step.text, citation.label());
                    }
                    None => {
                        let _ = writeln!(out, "{number}. {}", step.text);
                    }
                }
            }
        }
    }

    fn render_details(&self, out: &mut String) {
        if self.details.is_empty() {
            return;
        }
        out.push_str("\n## Details\n");
        for detail in &self.details {
            let _ = writeln!(out, "### {}", detail.component.as_str());
            if !detail.notes.is_empty() {
                let _ = writeln!(out, "{}", detail.notes);
            }
            if let Some(diagram) = &detail.diagram {
                let _ = writeln!(out, "{}", diagram.text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{BriefTopic, Citation, parse};

    #[test]
    fn citations_are_relative_line_ranges() {
        let citation = Citation::parse("src/arena.rs:42-58");
        assert_eq!(
            citation,
            Some(Citation {
                path: "src/arena.rs".into(),
                first_line: 42,
                last_line: 58
            })
        );
        assert_eq!(
            Citation::parse("`./lib.rs:L7`").map(|citation| citation.label()),
            Some("lib.rs:7".into())
        );
        assert_eq!(
            Citation::parse("../../store/log.rs:3").map(|citation| citation.path),
            Some("../../store/log.rs".into()),
            "elsewhere in the repository: read before it is kept"
        );
        for bad in [
            "/etc/passwd:1",
            "..:3",
            "../..:3",
            "a/../b.rs:3",
            "../a/../b.rs:3",
            "src/a.rs:0",
            "src/a.rs:9-3",
            "src/a.rs",
            "a b.rs:3",
            "src/a.rs:x",
        ] {
            assert_eq!(Citation::parse(bad), None, "{bad}");
        }
        let wide = Citation::parse("big.rs:10-900").map(|citation| citation.last_line);
        assert_eq!(wide, Some(49));
    }

    #[test]
    fn topics_key_the_cache() {
        assert_eq!(BriefTopic::Overview.key(), "overview");
        assert_eq!(
            BriefTopic::component("The Arena").key(),
            "component-thearena"
        );
    }

    #[test]
    fn only_read_citations_are_kept() {
        let mut brief = parse(
            "# Brief: x\n## Overview\nIt allocates.\n## Flows\n### Alloc\n1. Takes a slot (src/a.rs:4)\n\
             ## Citations\n- src/a.rs:4\n- src/b.rs:9\n",
        );
        let read: BTreeSet<Citation> = Citation::parse("src/b.rs:9").into_iter().collect();
        assert_eq!(brief.keep_citations(&read), 2);
        assert_eq!(brief.citations.len(), 1);
        assert_eq!(brief.flows[0].steps[0].citation, None);
        assert_eq!(brief.flows[0].steps[0].text, "Takes a slot");
    }
}
