//! What the repo tools returned this session, so a module map draws only real folders and files
//! (D-180, law 6). After a single `list_dir`, gemma4 drew modules the repo doesn't have.
//!
//! Every path a repo tool returned is kept with its folders: a listing's entries under the
//! folder listed, a file read, a search hit's file. Once any came back (a repo session), a node
//! whose label reads as a repo path (`crates/core`, `auth/`, `main.rs`) must name one of them:
//! the whole path, a tail of one at a folder boundary (`core/src`), or a single name one of
//! them has (`main.rs`). Otherwise it is drawn as a note marked "?" and the talker is told.
//! In a reply that read the repo (a tour), a label that is one lowercase identifier (`engine`,
//! `utils`) names a module too and is held to the same rule: gpt-oss-120b drew `state`, `engine`
//! and `utils` inside a folder it never listed. Labels in words ("Narration parser", "Redis")
//! are concepts and are left alone, so the prompt asks for real paths as labels.

use std::collections::BTreeSet;

use crate::engine::Core;
use crate::limits::REPO_NAMES_MAX;
use crate::narration::diagram::{DiagramSpec, SpecPart};
use crate::protocol::NodeShape;
use crate::tools::ToolOutput;

/// What a label that names no returned path gets after it, drawn as a note.
pub const UNVERIFIED_MARK: &str = " ?";

/// File extensions that make a lowercase `name.ext` read as a file.
const FILE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "py", "rb", "java", "kt", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "php", "scala", "md", "toml", "json", "yaml", "yml", "sql", "sh", "css",
    "html", "proto", "lock", "txt", "xml", "gradle", "vue", "svelte",
];

/// Whether a label names something the repo tools returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Not a repo session, or the label doesn't read as a path.
    Unchecked,
    Known,
    /// It reads as this path, which no repo tool returned.
    Unknown(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RepoNames {
    /// Returned paths and their folders, lowercase, without `./` or a trailing `/`.
    paths: BTreeSet<String>,
    /// The single names they are made of.
    names: BTreeSet<String>,
    /// A repo tool returned something this session.
    seen: bool,
    /// Past `REPO_NAMES_MAX` nothing more is kept, so labels are no longer checked: a real
    /// name that wasn't kept must never be marked as invented.
    full: bool,
}

impl RepoNames {
    /// A repo tool answered; `listed` is the folder a listing was asked for.
    pub(super) fn record(&mut self, listed: Option<&str>, output: &ToolOutput) {
        match output {
            ToolOutput::File { path, .. } => self.add(path),
            ToolOutput::Matches { hits, .. } => {
                for hit in hits {
                    self.add(&hit.path);
                }
            }
            ToolOutput::Listing { entries, .. } => {
                let folder = listed.map(normalized).unwrap_or_default();
                for entry in entries {
                    let path = if folder.is_empty() || folder == "." {
                        entry.name.clone()
                    } else {
                        format!("{folder}/{}", entry.name)
                    };
                    self.add(&path);
                }
            }
            ToolOutput::Started { .. }
            | ToolOutput::Text { .. }
            | ToolOutput::Failed { .. }
            | ToolOutput::View { .. } => return,
        }
        self.seen = true;
    }

    /// The paths a research brief's checked citations name (D-252): its diagram is harness
    /// output, and its labels are held to what the core read of it. With none, every label that
    /// reads as a path names something unread.
    pub(super) fn of_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        let mut names = Self {
            seen: true,
            ..Self::default()
        };
        for path in paths {
            names.add(path);
        }
        names
    }

    fn add(&mut self, path: &str) {
        let path = normalized(path);
        let mut folder = String::new();
        for name in path.split('/').filter(|name| !name.is_empty()) {
            if self.paths.len() >= REPO_NAMES_MAX {
                self.full = true;
                return;
            }
            if !folder.is_empty() {
                folder.push('/');
            }
            folder.push_str(name);
            self.names.insert(name.to_owned());
            self.paths.insert(folder.clone());
        }
    }

    /// Whether `label` names a returned path, when it reads as one. In a tour (`touring`) a
    /// single lowercase identifier reads as a module's name too, and labels are checked even
    /// when nothing came back yet: a tour whose reads were written instead of called has read
    /// nothing, so every module it names is unverified.
    pub(super) fn verdict(&self, label: &str, touring: bool) -> Verdict {
        if self.full || !(self.seen || touring) {
            return Verdict::Unchecked;
        }
        let Some(path) = path_in(label).or_else(|| touring.then(|| identifier(label))?) else {
            return Verdict::Unchecked;
        };
        let known = self.paths.contains(&path)
            || (!path.contains('/') && self.names.contains(&path))
            || self
                .paths
                .iter()
                .any(|known| known.ends_with(&format!("/{path}")));
        if known {
            Verdict::Known
        } else {
            Verdict::Unknown(path)
        }
    }
}

impl Core {
    /// A node's label and kind as drawn: a label naming a repo path no tool returned becomes a
    /// note marked "?", and the talker is told.
    pub(super) fn checked_label(
        &mut self,
        label: &str,
        shape: Option<NodeShape>,
    ) -> (String, Option<NodeShape>) {
        let touring = self.reply.as_ref().is_some_and(|reply| reply.touring);
        let Verdict::Unknown(path) = self.repo_names.verdict(label, touring) else {
            return (label.to_owned(), shape);
        };
        self.context.notice(format!(
            "\"{label}\" was drawn as a note marked \"?\": no repo tool returned {path} this \
             session. Draw only folders and files a tool returned, with their real names; list or \
             read first."
        ));
        (format!("{label}{UNVERIFIED_MARK}"), Some(NodeShape::Note))
    }

    /// A staged diagram's labels, held to what was read as a sketch's are (D-180, D-252): a
    /// container, box, record or row whose label names a repo path no tool returned is marked
    /// " ?" where it stands, and the talker is told once for the diagram. Live, gemma4 staged a
    /// repo map before reading anything, with `executor.rs`, `state_manager.rs` and
    /// `task_queue.rs`, none of them in the repo (T-035).
    pub(super) fn checked_spec(&mut self, spec: &DiagramSpec) -> DiagramSpec {
        let touring = self.reply.as_ref().is_some_and(|reply| reply.touring);
        let names = &self.repo_names;
        let (checked, unknown) = marked_unread(spec, |label| names.verdict(label, touring));
        if !unknown.is_empty() {
            self.context.notice(format!(
                "In your diagram \"{}\", {} marked \"?\": no repo tool returned {} this session. \
                 Draw only folders and files a tool returned, with their real names; list or read \
                 first.",
                spec.title,
                parts_were(unknown.len()),
                unknown.join(", ")
            ));
        }
        checked
    }
}

/// `spec` with each container, box, record or row whose label `verdict` finds unread marked
/// " ?", and the paths they named. The part keeps its kind and place: a container made a note
/// would leave what it holds without a frame.
pub(super) fn marked_unread(
    spec: &DiagramSpec,
    verdict: impl Fn(&str) -> Verdict,
) -> (DiagramSpec, Vec<String>) {
    let mut checked = spec.clone();
    let mut unknown = Vec::new();
    for item in &mut checked.items {
        let label = match &mut item.part {
            SpecPart::Container { label, .. }
            | SpecPart::Node { label, .. }
            | SpecPart::Record { label, .. }
            | SpecPart::Cells { label, .. } => label,
            SpecPart::Note { .. } | SpecPart::Edge { .. } => continue,
        };
        if let Verdict::Unknown(path) = verdict(label) {
            unknown.push(path);
            label.push_str(UNVERIFIED_MARK);
        }
    }
    (checked, unknown)
}

/// "1 part was", "3 parts were".
pub(super) fn parts_were(count: usize) -> String {
    if count == 1 {
        "1 part was".to_owned()
    } else {
        format!("{count} parts were")
    }
}

/// Lowercase, with `\` as `/`, no `./` in front and no `/` behind.
fn normalized(path: &str) -> String {
    let path = path.trim().replace('\\', "/").to_lowercase();
    let path = path.strip_prefix("./").unwrap_or(&path);
    path.trim_end_matches('/').to_owned()
}

/// A label that is one lowercase identifier (`engine`, `user_store`, `api-gateway`).
fn identifier(label: &str) -> Option<String> {
    let label = label.trim();
    let mut chars = label.chars();
    let first = chars.next()?;
    let rest_ok = chars
        .clone()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
    (first.is_ascii_lowercase() && rest_ok && chars.next().is_some()).then(|| label.to_owned())
}

/// The first word of a label that reads as a repo path: one with a `/` between names or after
/// one (`crates/core`, `auth/`), or a lowercase `name.ext` with a source file's extension.
fn path_in(label: &str) -> Option<String> {
    label.split_whitespace().find_map(|word| {
        let word = word.trim_matches(|ch: char| "()[]{},;:\"'`".contains(ch));
        if word.contains("://") {
            return None;
        }
        let path = normalized(word);
        let has_names = path.split('/').all(|name| !name.is_empty() && name != "..");
        let folders = word.contains(['/', '\\']) && has_names && !path.is_empty();
        let file = word.rsplit_once('.').is_some_and(|(name, extension)| {
            !name.is_empty()
                && !name.contains('/')
                && name.chars().all(|ch| !ch.is_uppercase())
                && FILE_EXTENSIONS.contains(&extension)
        });
        (folders || file).then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::{RepoNames, Verdict};
    use crate::limits::REPO_NAMES_MAX;
    use crate::tools::{DirEntry, SearchHit, ToolOutput};

    fn listing(names: &[&str]) -> ToolOutput {
        ToolOutput::Listing {
            entries: names
                .iter()
                .map(|name| DirEntry {
                    name: (*name).to_owned(),
                    is_dir: true,
                })
                .collect(),
            more_entries: 0,
        }
    }

    fn unknown(path: &str) -> Verdict {
        Verdict::Unknown(path.to_owned())
    }

    /// The owner's tour: one `list_dir` of the root, then modules it never returned.
    #[test]
    fn only_returned_paths_are_known() {
        let mut names = RepoNames::default();
        assert_eq!(
            names.verdict("crates/auth", false),
            Verdict::Unchecked,
            "no repo yet"
        );
        assert_eq!(
            names.verdict("internal", true),
            unknown("internal"),
            "a tour that read nothing has seen nothing"
        );
        names.record(Some("."), &listing(&["crates", "docs", "ui"]));
        assert_eq!(names.verdict("crates/", false), Verdict::Known);
        assert_eq!(names.verdict("./ui", false), Verdict::Known);
        assert_eq!(names.verdict("crates/auth", false), unknown("crates/auth"));
        assert_eq!(names.verdict("auth/", false), unknown("auth"));
        assert_eq!(names.verdict("Narration parser", false), Verdict::Unchecked);
        assert_eq!(names.verdict("Node.js runtime", false), Verdict::Unchecked);
        names.record(Some("crates/"), &listing(&["core", "runtime"]));
        assert_eq!(names.verdict("crates/core", false), Verdict::Known);
        assert_eq!(names.verdict("Core (crates/core)", false), Verdict::Known);
        names.record(
            None,
            &ToolOutput::File {
                path: "crates/core/src/lib.rs".to_owned(),
                first_line: 1,
                lines: Vec::new(),
                more_lines: 0,
            },
        );
        assert_eq!(
            names.verdict("core/src", false),
            Verdict::Known,
            "a tail at a folder"
        );
        assert_eq!(
            names.verdict("lib.rs", false),
            Verdict::Known,
            "a single name"
        );
        assert_eq!(names.verdict("main.rs", false), unknown("main.rs"));
        assert_eq!(
            names.verdict("ore/src", false),
            unknown("ore/src"),
            "not mid-name"
        );
        names.record(
            None,
            &ToolOutput::Matches {
                hits: vec![SearchHit {
                    path: "ui/src/main.tsx".to_owned(),
                    line: 3,
                    text: String::new(),
                }],
                more_hits: 0,
            },
        );
        assert_eq!(names.verdict("ui/src/main.tsx", false), Verdict::Known);
        // In a tour, one lowercase word is a module's name too.
        assert_eq!(names.verdict("engine", false), Verdict::Unchecked);
        assert_eq!(names.verdict("engine", true), unknown("engine"));
        assert_eq!(names.verdict("runtime", true), Verdict::Known);
        assert_eq!(names.verdict("Redis", true), Verdict::Unchecked);
        assert_eq!(
            names.verdict("x", true),
            Verdict::Unchecked,
            "too short to be a name"
        );
    }

    /// Past the bound nothing is checked: a real name that wasn't kept is never marked.
    #[test]
    fn past_the_bound_labels_are_not_checked() {
        let mut names = RepoNames::default();
        let many: Vec<String> = (0..=REPO_NAMES_MAX)
            .map(|index| format!("m{index}"))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        names.record(Some("."), &listing(&many));
        assert_eq!(names.verdict("crates/auth", false), Verdict::Unchecked);
    }
}
