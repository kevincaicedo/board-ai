//! The brief parser: one line at a time through an explicit section state, never recursive, and
//! bounded at every step (the whole text, each line, each section, each list). What doesn't fit
//! the shape is dropped and counted in [`BriefDropped`](super::BriefDropped).
//!
//! The whole text is parsed at once: a brief arrives in one piece, as the harness's final answer,
//! so there is no stream to split.

use std::collections::BTreeSet;

use super::{Brief, BriefDiagram, Citation, Component, Detail, Flow, FlowStep, Relation};
use crate::ids::ElementId;
use crate::limits::{
    BRIEF_CITATIONS_MAX, BRIEF_COMPONENTS_MAX, BRIEF_DETAIL_NOTES_MAX_BYTES, BRIEF_DETAILS_MAX,
    BRIEF_DIAGRAM_MAX_BYTES, BRIEF_FIELD_MAX_BYTES, BRIEF_FLOW_STEPS_MAX, BRIEF_FLOWS_MAX,
    BRIEF_LINE_MAX_BYTES, BRIEF_MAX_BYTES, BRIEF_OVERVIEW_MAX_BYTES, BRIEF_RELATIONS_MAX,
    BRIEF_TOPIC_MAX_BYTES, TITLE_MAX_BYTES,
};
use crate::narration::tags::bounded;
use crate::untrusted::PlainText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// Before the first `##`: the `# Brief:` title, and chatter that is dropped.
    Preamble,
    Overview,
    Components,
    Relations,
    Flows,
    Diagram,
    Details,
    Citations,
    /// A `##` heading the shape doesn't have: its lines are dropped.
    Unknown,
}

/// A `<diagram>` block being read, and whether it belongs to the current detail.
struct Block {
    text: String,
    title: String,
    for_detail: bool,
}

#[derive(Default)]
struct Parser {
    brief: Brief,
    section: Option<Section>,
    block: Option<Block>,
    /// Relations and details name components, which may be listed after them: they are kept
    /// raw until the end and checked then.
    relations: Vec<Relation>,
    details: Vec<Detail>,
    /// The current flow or detail heading was refused: its lines go with it.
    skipping: bool,
}

/// Parses a brief. Never fails: an answer with nothing usable gives an empty brief whose
/// [`Brief::is_usable`] is false, with the drops counted.
#[must_use]
pub fn parse(text: &str) -> Brief {
    let mut parser = Parser::default();
    let (text, cut) = cut_at_line(text, BRIEF_MAX_BYTES);
    parser.brief.dropped.truncated |= cut;
    for line in text.lines() {
        parser.line(line);
    }
    parser.finish()
}

/// `line` cut at a character within `max_bytes`.
fn cut_at_char(line: &str, max_bytes: usize) -> (&str, bool) {
    if line.len() <= max_bytes {
        return (line, false);
    }
    let mut end = max_bytes;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    (&line[..end], true)
}

/// `text` cut at the last line end within `max_bytes`.
fn cut_at_line(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').unwrap_or(0);
    (&text[..end], true)
}

impl Parser {
    fn line(&mut self, line: &str) {
        // A paragraph written on one long line keeps its start rather than vanishing.
        let (line, cut) = cut_at_char(line, BRIEF_LINE_MAX_BYTES);
        self.brief.dropped.truncated |= cut;
        let trimmed = line.trim();
        if self.block.is_some() {
            if !trimmed.starts_with("##") {
                self.block_line(line);
                return;
            }
            // A heading ends a block that lost its closing tag, so it can't swallow the
            // sections after it (the digest parser does the same).
            self.unclosed_block();
        }
        if trimmed.is_empty() {
            return;
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            self.section = Some(section_named(heading));
            self.skipping = false;
            if self.section == Some(Section::Unknown) {
                self.brief.dropped.lines += 1;
            }
            return;
        }
        if let Some(heading) = trimmed.strip_prefix("### ") {
            self.subheading(heading.trim());
            return;
        }
        if trimmed.starts_with("<diagram") {
            self.open_block(trimmed);
            return;
        }
        if self.skipping {
            return;
        }
        self.content(trimmed);
    }

    fn content(&mut self, line: &str) {
        let dropped = &mut self.brief.dropped;
        match self.section.unwrap_or(Section::Preamble) {
            Section::Preamble => match line.strip_prefix("# ") {
                Some(title) if self.brief.topic.is_empty() => {
                    let title = strip_label(title.trim(), "brief:");
                    self.brief.topic = bounded(title, BRIEF_TOPIC_MAX_BYTES);
                }
                Some(_) | None => dropped.lines += 1,
            },
            Section::Overview => {
                if !append(&mut self.brief.overview, line, BRIEF_OVERVIEW_MAX_BYTES) {
                    dropped.truncated = true;
                }
            }
            Section::Components => self.component(line),
            Section::Relations => self.relation(line),
            Section::Flows => self.step(line),
            Section::Details => match self.details.last_mut() {
                Some(detail) => {
                    if !append(&mut detail.notes, line, BRIEF_DETAIL_NOTES_MAX_BYTES) {
                        dropped.truncated = true;
                    }
                }
                None => dropped.lines += 1,
            },
            Section::Citations => self.citation(line),
            Section::Diagram | Section::Unknown => dropped.lines += 1,
        }
    }

    fn subheading(&mut self, heading: &str) {
        self.skipping = false;
        match self.section.unwrap_or(Section::Preamble) {
            Section::Flows if self.brief.flows.len() < BRIEF_FLOWS_MAX => {
                self.brief.flows.push(Flow {
                    name: bounded(heading, TITLE_MAX_BYTES),
                    steps: Vec::new(),
                });
            }
            Section::Flows => {
                self.brief.dropped.flows += 1;
                self.skipping = true;
            }
            Section::Details => match element_id(heading) {
                Some(component) if self.details.len() < BRIEF_DETAILS_MAX => {
                    self.details.push(Detail {
                        component,
                        notes: String::new(),
                        diagram: None,
                    });
                }
                Some(_) | None => {
                    self.brief.dropped.details += 1;
                    self.skipping = true;
                }
            },
            Section::Preamble
            | Section::Overview
            | Section::Components
            | Section::Relations
            | Section::Diagram
            | Section::Citations
            | Section::Unknown => self.brief.dropped.lines += 1,
        }
    }

    fn component(&mut self, line: &str) {
        let parsed = bullet(line).and_then(component);
        let full = self.brief.components.len() >= BRIEF_COMPONENTS_MAX;
        match parsed {
            Some(component)
                if !full
                    && !self
                        .brief
                        .components
                        .iter()
                        .any(|known| known.id == component.id) =>
            {
                self.brief.components.push(component);
            }
            Some(_) | None => self.brief.dropped.components += 1,
        }
    }

    fn relation(&mut self, line: &str) {
        match bullet(line).and_then(relation) {
            Some(relation) if self.relations.len() < BRIEF_RELATIONS_MAX => {
                self.relations.push(relation);
            }
            Some(_) | None => self.brief.dropped.relations += 1,
        }
    }

    fn step(&mut self, line: &str) {
        let Some(text) = numbered(line) else {
            self.brief.dropped.lines += 1;
            return;
        };
        let Some(flow) = self.brief.flows.last_mut() else {
            self.brief.dropped.steps += 1;
            return;
        };
        if flow.steps.len() >= BRIEF_FLOW_STEPS_MAX {
            self.brief.dropped.steps += 1;
            return;
        }
        let (text, citation) = trailing_citation(text);
        flow.steps.push(FlowStep {
            text: bounded(text, BRIEF_FIELD_MAX_BYTES),
            citation,
        });
    }

    fn citation(&mut self, line: &str) {
        let parsed = bullet(line).and_then(Citation::parse);
        match parsed {
            Some(citation) if self.brief.citations.len() < BRIEF_CITATIONS_MAX => {
                if !self.brief.citations.contains(&citation) {
                    self.brief.citations.push(citation);
                }
            }
            Some(_) | None => self.brief.dropped.citations += 1,
        }
    }

    /// `<diagram title="…">` opens a block in the Diagram section, or under a detail.
    fn open_block(&mut self, line: &str) {
        let for_detail = match self.section {
            Some(Section::Diagram) => false,
            Some(Section::Details) if !self.details.is_empty() && !self.skipping => true,
            Some(_) | None => {
                self.brief.dropped.lines += 1;
                return;
            }
        };
        let title = attribute(line, "title").unwrap_or_default();
        self.block = Some(Block {
            text: String::new(),
            title: bounded(title, TITLE_MAX_BYTES),
            for_detail,
        });
        self.block_line(line);
    }

    fn block_line(&mut self, line: &str) {
        let Some(block) = self.block.as_mut() else {
            return;
        };
        let room = BRIEF_DIAGRAM_MAX_BYTES.saturating_sub(block.text.len());
        if line.len() < room {
            block.text.push_str(line);
            block.text.push('\n');
        } else {
            self.brief.dropped.truncated = true;
        }
        if line.contains("</diagram>") {
            self.close_block();
        }
    }

    fn close_block(&mut self) {
        let Some(block) = self.block.take() else {
            return;
        };
        let diagram = BriefDiagram {
            title: block.title,
            text: block.text.trim_end().to_owned(),
            ids: Vec::new(),
        };
        if block.for_detail {
            if let Some(detail) = self.details.last_mut() {
                detail.diagram = Some(diagram);
            }
        } else if self.brief.diagram.is_none() {
            self.brief.diagram = Some(diagram);
        } else {
            self.brief.dropped.lines += 1;
        }
    }

    /// A block whose closing tag never came keeps what it had, closed: its end was lost.
    fn unclosed_block(&mut self) {
        let Some(block) = self.block.as_mut() else {
            return;
        };
        if block.text.len() + "</diagram>".len() <= BRIEF_DIAGRAM_MAX_BYTES {
            block.text.push_str("</diagram>");
        }
        self.brief.dropped.truncated = true;
        self.close_block();
    }

    /// Checks what names components against the final list, and fills each diagram's ids.
    fn finish(mut self) -> Brief {
        self.unclosed_block();
        let known: BTreeSet<ElementId> = self
            .brief
            .components
            .iter()
            .map(|component| component.id.clone())
            .collect();
        for relation in std::mem::take(&mut self.relations) {
            if known.contains(&relation.from) && known.contains(&relation.to) {
                self.brief.relations.push(relation);
            } else {
                self.brief.dropped.relations += 1;
            }
        }
        for mut detail in std::mem::take(&mut self.details) {
            if known.contains(&detail.component) {
                if let Some(diagram) = detail.diagram.as_mut() {
                    diagram.ids = diagram_ids(&diagram.text, &known);
                }
                self.brief.details.push(detail);
            } else {
                self.brief.dropped.details += 1;
            }
        }
        if let Some(diagram) = self.brief.diagram.as_mut() {
            diagram.ids = diagram_ids(&diagram.text, &known);
        }
        self.brief
    }
}

fn section_named(heading: &str) -> Section {
    let name = heading.trim().trim_end_matches(':').to_ascii_lowercase();
    match name.as_str() {
        "overview" => Section::Overview,
        "components" => Section::Components,
        "relations" => Section::Relations,
        "flows" => Section::Flows,
        "diagram" => Section::Diagram,
        "details" => Section::Details,
        "citations" | "sources" => Section::Citations,
        _ => Section::Unknown,
    }
}

/// Appends a line to a paragraph within `max_bytes`; false when it didn't fit.
fn append(target: &mut String, line: &str, max_bytes: usize) -> bool {
    let extra = usize::from(!target.is_empty());
    if target.len() + extra + line.len() > max_bytes {
        return false;
    }
    if extra == 1 {
        target.push(' ');
    }
    target.push_str(line);
    true
}

/// `text` after a case-insensitive label ("Brief: x" → "x"), else `text`.
fn strip_label<'a>(text: &'a str, label: &str) -> &'a str {
    match text.get(..label.len()) {
        Some(head) if head.eq_ignore_ascii_case(label) => text[label.len()..].trim(),
        Some(_) | None => text,
    }
}

/// A list item's text without its bullet.
fn bullet(line: &str) -> Option<&str> {
    ["- ", "* ", "• "]
        .iter()
        .find_map(|mark| line.strip_prefix(mark))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
}

/// A numbered item's text: `1. text` or `1) text`.
fn numbered(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = line[digits..].strip_prefix(['.', ')'])?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// A step's words and the citation in parentheses at its end, if it reads as one.
fn trailing_citation(text: &str) -> (&str, Option<Citation>) {
    let Some(inner) = text.strip_suffix(')') else {
        return (text, None);
    };
    let Some(open) = inner.rfind('(') else {
        return (text, None);
    };
    match Citation::parse(&inner[open + 1..]) {
        Some(citation) => (inner[..open].trim_end(), Some(citation)),
        None => (text, None),
    }
}

/// An id as the harness wrote it, backticks aside.
fn element_id(text: &str) -> Option<ElementId> {
    ElementId::parse(text.trim().trim_matches('`'))
}

fn field(text: &str) -> String {
    bounded(text.trim().trim_matches('`'), BRIEF_FIELD_MAX_BYTES)
}

/// `<id> · <name> · <kind> · <path> — <role>`; `|` also separates fields, and the role may follow
/// `—`, `–`, `--` or ` - `. The id and the name are required.
fn component(text: &str) -> Option<Component> {
    let (fields, role) = [" — ", " – ", " -- ", " - "]
        .iter()
        .find_map(|dash| text.split_once(dash))
        .unwrap_or((text, ""));
    let mut parts = fields.split(['·', '|']).map(str::trim);
    let id = element_id(parts.next()?)?;
    // BoardAI's notes may name a part by its name: plain text, parsed here (D-266).
    let name =
        PlainText::parse(parts.next()?.trim_matches('`'), BRIEF_FIELD_MAX_BYTES).into_string();
    if name.is_empty() {
        return None;
    }
    Some(Component {
        id,
        name,
        kind: field(parts.next().unwrap_or_default()),
        path: field(parts.next().unwrap_or_default()),
        role: field(role),
    })
}

/// `<id> → <id>: <label>` (or `->`).
fn relation(text: &str) -> Option<Relation> {
    let (from, rest) = text.split_once('→').or_else(|| text.split_once("->"))?;
    let (to, label) = rest.split_once(':').unwrap_or((rest, ""));
    Some(Relation {
        from: element_id(from)?,
        to: element_id(to)?,
        label: field(label),
    })
}

/// The value of `name="…"` in a tag line.
fn attribute<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let start = line.find(&marker)? + marker.len();
    let end = line[start..].find('"')?;
    Some(&line[start..start + end])
}

/// Component ids a diagram's tags name, through `id`, `from`, `to`, `in` or `parent`, in order.
fn diagram_ids(text: &str, known: &BTreeSet<ElementId>) -> Vec<ElementId> {
    let mut ids: Vec<ElementId> = Vec::new();
    for marker in [" id=\"", " from=\"", " to=\"", " in=\"", " parent=\""] {
        let mut rest = text;
        while let Some(start) = rest.find(marker) {
            let after = &rest[start + marker.len()..];
            let Some(end) = after.find('"') else {
                break;
            };
            if let Some(id) = ElementId::parse(&after[..end])
                && known.contains(&id)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
            rest = &after[end..];
        }
    }
    let position = |id: &ElementId| text.find(&format!("\"{}\"", id.as_str()));
    ids.sort_by_key(position);
    ids
}
