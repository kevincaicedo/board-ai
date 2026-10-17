//! Sources (PRODUCT §7.7, D-184): what the user shared for BoardAI to read, for this session or
//! kept in a project, and the research briefs a coding harness writes about code (D-185).
//!
//! The runtime validates every path where it enters (a native drop, the macOS chooser, or a
//! name said by voice and resolved inside the allowed code folders) and gives the core only what
//! it may show: a [`SourceInfo`] with an id, a kind, a title and a `~/…` path. The core decides
//! everything else: the scope, the current project, when research starts, what the talker is
//! told, and what the tray shows. No absolute path ever reaches the core or the UI.

pub mod brief;
pub mod cues;
pub mod state;
pub mod tray;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::TaskId;
use crate::limits::{
    PROJECT_NAME_MAX_BYTES, SOURCE_SHOWN_PATH_MAX_BYTES, SOURCE_TITLE_MAX_BYTES,
    SPOKEN_SOURCE_NAME_MAX_BYTES,
};
use crate::narration::tags::bounded;

pub use brief::{Brief, BriefTopic, Citation};

/// A source, numbered by the runtime when it registers a validated path. Ids are never reused
/// within a run of the app, so a late answer about a removed source names nothing.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct SourceId(pub u32);

/// What a source is, decided by the runtime from the folder or the file's extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum SourceKind {
    /// A codebase, a crate or a module folder.
    CodeFolder,
    /// One source file.
    CodeFile,
    /// A document to read (documents arrive with T-031; until then they wait).
    Document { format: DocumentFormat },
}

impl SourceKind {
    /// Code is understood through the coding harness (D-185).
    #[must_use]
    pub const fn is_code(self) -> bool {
        match self {
            Self::CodeFolder | Self::CodeFile => true,
            Self::Document { .. } => false,
        }
    }

    /// How BoardAI names the kind out loud and in the vault.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::CodeFolder => "code folder",
            Self::CodeFile => "code file",
            Self::Document {
                format: DocumentFormat::Markdown,
            } => "Markdown document",
            Self::Document {
                format: DocumentFormat::Text,
            } => "text document",
            Self::Document {
                format: DocumentFormat::Pdf,
            } => "PDF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DocumentFormat {
    Markdown,
    Text,
    Pdf,
}

/// A project's id: the slug of its name, which names its vault file (`projects/<id>.md`, the
/// tasks) and folder (`projects/<id>/`, its sources and their briefs). Lowercase ASCII letters
/// and digits in words joined by `-`, at most [`PROJECT_NAME_MAX_BYTES`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectId(String);

impl ProjectId {
    /// Parses a slug as it is written; anything else is refused.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid = !text.is_empty()
            && text.len() <= PROJECT_NAME_MAX_BYTES
            && text.split('-').all(|word| {
                !word.is_empty()
                    && word
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            });
        valid.then(|| Self(text.to_owned()))
    }

    /// The slug of a name ("InfinityDB" → `infinitydb`, "the harness project" → `the-harness-project`);
    /// `None` when the name has no letters or digits.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let mut slug = String::new();
        for word in name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
        {
            let word = word.to_ascii_lowercase();
            let extra = usize::from(!slug.is_empty());
            if slug.len() + extra + word.len() > PROJECT_NAME_MAX_BYTES {
                break;
            }
            if extra == 1 {
                slug.push('-');
            }
            slug.push_str(&word);
        }
        Self::parse(&slug)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProjectId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| format!("invalid project id: {text:?}"))
    }
}

impl From<ProjectId> for String {
    fn from(id: ProjectId) -> Self {
        id.0
    }
}

/// A project (PRODUCT §7.5): its id and its name as the user said it or the tray shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
}

impl Project {
    /// A project named `name`; `None` when the name has nothing to make a slug from.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        let name = bounded(name, PROJECT_NAME_MAX_BYTES);
        Some(Self {
            id: ProjectId::from_name(&name)?,
            name,
        })
    }
}

/// A source's state in the tray (artboard 14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state")]
#[ts(export)]
pub enum SourceState {
    /// Being read: `reader` (the harness, "Claude Code") has read `files_read` files so far, of
    /// about `files_estimate` when the runtime counted them.
    Reading {
        reader: String,
        files_read: u32,
        files_estimate: Option<u32>,
    },
    /// What BoardAI learned is ready ("Overview ready · 17 parts mapped").
    Ready { detail: String },
    /// Ready, but something couldn't be read ("3 figures unread").
    Partial { reason: String },
    /// Nothing could be read.
    Failed { reason: String },
    /// Registered, with nothing to read it yet ("Waiting for the document reader").
    Waiting { reason: String },
}

/// A source as the runtime registered it: everything the core and the UI may know of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: SourceId,
    pub kind: SourceKind,
    /// Its folder or file name ("inf-alloc", "hekaton-sigmod13.pdf").
    pub title: String,
    /// Its path under the home folder, shortened for the tray (`~/…/infinitydb/crates/inf-alloc`).
    pub shown_path: String,
    /// Files a bounded walk counted in a code folder, for "23 of ~40 files".
    pub files_estimate: Option<u32>,
}

impl SourceInfo {
    /// The same, held to the core's bounds: it came from the runtime.
    #[must_use]
    pub fn bounded(self) -> Self {
        Self {
            title: bounded(&self.title, SOURCE_TITLE_MAX_BYTES),
            shown_path: bounded(&self.shown_path, SOURCE_SHOWN_PATH_MAX_BYTES),
            ..self
        }
    }
}

/// A source kept in a project, as the runtime read it from the project's vault folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptSource {
    pub source: SourceInfo,
    /// The overview brief the cache holds for it, when it is still fresh (the git HEAD and the
    /// files' modification times match what it was written from): its parts, and the day.
    pub overview: Option<CachedOverview>,
    /// Kept, but its path is gone or no longer passes the checks: shown failed, never read.
    pub missing: bool,
}

/// What the tray and "more detail on X" need from a cached overview brief, without its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedOverview {
    /// The brief's components, `(id, name)`, at most `BRIEF_COMPONENTS_MAX`.
    pub components: Vec<(String, String)>,
    /// The day it was written, `YYYY-MM-DD`.
    pub written: String,
}

/// Lines of a cited file the runtime read natively, inside the source (law 6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitedLines {
    /// The path as the brief cited it, relative to the source's folder.
    pub path: String,
    pub first_line: u32,
    pub lines: Vec<String>,
}

/// Which half of the window a drag is over (14b): the left shares for this session, the right
/// keeps in the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DropHalf {
    Session,
    Project,
}

/// A drag over the window, from the app's native drop events: what it would share, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DragOver {
    /// The first dragged item's name, or "3 items".
    pub title: String,
    /// Why it can't be shared, when the app refused every item.
    pub refused: Option<String>,
    pub half: DropHalf,
}

/// What the runtime could not do; the user hears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFailure {
    /// A dropped or chosen path didn't pass the checks.
    Refused,
    /// Keeping a source in the project, or stopping, wasn't written.
    NotKept,
    /// A project's kept sources couldn't be read.
    ProjectUnreadable,
}

/// What the runtime tells the core about sources (LLD §2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source_input", rename_all = "snake_case")]
pub enum SourceInput {
    /// Once, as the session opens: the harness that researches code (`None` when none is
    /// installed), and the project the last session worked on, whose kept sources follow as
    /// `ProjectLoaded`.
    Opened {
        harness: Option<String>,
        project: Option<Project>,
    },
    /// The user shared a path natively (a drop or the chooser): validated, registered, and
    /// readable for this session from now on. `keep`: dropped on the right half.
    Shared { source: SourceInfo, keep: bool },
    /// The answer to `Resolve`: the matches for the spoken name, each registered but not yet
    /// readable; `searched` counts the folders the bounded walk looked in.
    Resolved {
        request: u32,
        candidates: Vec<SourceInfo>,
        searched: u32,
    },
    /// The answer to `SetProject`: the sources the project keeps (`left_out` past the bound).
    ProjectLoaded {
        project: Project,
        kept: Vec<KeptSource>,
        left_out: u32,
    },
    /// The answer to `Brief`: the cache held a fresh brief for that source and topic.
    BriefCached { task: TaskId, markdown: String },
    /// The answer to `Brief`: nothing fresh was cached, so the harness started on the prompt;
    /// its events follow as `Input::Harness` for the same task.
    BriefStarted { task: TaskId },
    /// The answer to `CheckCitations`: the cited lines that were read, and how many citations
    /// named nothing readable.
    CitationsChecked {
        task: TaskId,
        read: Vec<CitedLines>,
        unverified: Vec<Citation>,
    },
    /// A drag over the window; `None` once it left or dropped.
    Dragging { drag: Option<DragOver> },
    /// Something the runtime couldn't do.
    Failed {
        failure: SourceFailure,
        detail: String,
    },
    /// A document reader's report (the `SourceReader` seam; T-031 brings the readers): reading,
    /// ready, partial with what it couldn't read, or failed. A ready or partial report may carry
    /// the document's digest in Markdown, bounded by `DOCUMENT_DIGEST_MAX_BYTES`.
    DocumentRead {
        source: SourceId,
        state: SourceState,
        digest: Option<String>,
    },
}

/// What a spoken name was called ("the inf-alloc **crate**"), which steers the resolver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameKind {
    /// A crate, package, library or repo: a folder, preferably one with a manifest.
    Package,
    /// A module: a folder or a source file of that name.
    Module,
    /// A folder or directory.
    Folder,
    /// A file.
    File,
}

/// What the core asks the runtime to do about sources (LLD §2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source_effect", rename_all = "snake_case")]
pub enum SourceEffect {
    /// Find `name` inside the allowed code folders and the shared sources, with a bounded walk;
    /// answered by `Resolved`.
    Resolve {
        request: u32,
        name: String,
        kind: NameKind,
    },
    /// A resolved candidate becomes readable for this session.
    Share { source: SourceId },
    /// Forget these: candidates not chosen, a share past the bound, a removed source. Nothing of
    /// them stays readable.
    Discard { sources: Vec<SourceId> },
    /// The session works on this project now: load its kept sources (`ProjectLoaded`), and
    /// record it so the next session starts with it.
    SetProject { project: Project },
    /// Keep this source in the project's vault folder, or stop keeping it.
    Keep { source: SourceId, project: Project },
    Unkeep {
        source: SourceId,
        project: ProjectId,
    },
    /// A brief for this source and topic: the cache's when it is fresh (`BriefCached`), else run
    /// research on `prompt` in the source's folder (`BriefStarted`, then harness events).
    Brief {
        task: TaskId,
        source: SourceId,
        topic: BriefTopic,
        prompt: String,
    },
    /// Keep this parsed brief, in its bounded form, for the source and topic: in memory for a
    /// session source, in the project's vault folder for a kept one.
    CacheBrief {
        source: SourceId,
        topic: BriefTopic,
        markdown: String,
    },
    /// Read each cited range natively (bounded) inside the source; answered by
    /// `CitationsChecked`.
    CheckCitations {
        task: TaskId,
        source: SourceId,
        citations: Vec<Citation>,
    },
}

/// Letters and digits only, lowercase: how names are compared ("INF-Alloc", "inf alloc" and
/// "infalloc" are one name).
#[must_use]
pub fn name_key(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// A spoken name, held to its bound; `None` when nothing is left of it.
#[must_use]
pub fn spoken_name(text: &str) -> Option<String> {
    let text = text
        .trim()
        .trim_matches(|character: char| !character.is_alphanumeric());
    (!text.is_empty() && text.len() <= SPOKEN_SOURCE_NAME_MAX_BYTES && !name_key(text).is_empty())
        .then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Project, ProjectId, name_key, spoken_name};
    use crate::limits::PROJECT_NAME_MAX_BYTES;

    #[test]
    fn project_ids_are_slugs_parsed_at_the_boundary() {
        for good in ["infinitydb", "the-harness", "v2", "a-1-b"] {
            assert!(ProjectId::parse(good).is_some(), "{good}");
        }
        let long = "a".repeat(PROJECT_NAME_MAX_BYTES + 1);
        for bad in [
            "",
            "-a",
            "a-",
            "a--b",
            "Caps",
            "a b",
            "a/b",
            "..",
            long.as_str(),
        ] {
            assert!(ProjectId::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(
            ProjectId::from_name("InfinityDB").map(|id| id.as_str().to_owned()),
            Some("infinitydb".to_owned())
        );
        assert_eq!(
            Project::named("  The harness project ").map(|project| project.id.as_str().to_owned()),
            Some("the-harness-project".to_owned())
        );
        assert!(Project::named("…").is_none());
        assert!(serde_json::from_str::<ProjectId>("\"../etc\"").is_err());
    }

    #[test]
    fn names_compare_by_their_letters_and_digits() {
        assert_eq!(name_key("INF-Alloc"), "infalloc");
        assert_eq!(name_key("inf alloc"), name_key("inf_alloc"));
        assert_eq!(
            spoken_name(" \"inf-alloc\". "),
            Some("inf-alloc".to_owned())
        );
        assert_eq!(spoken_name(" -- "), None);
        assert_eq!(spoken_name(&"x".repeat(100)), None);
    }
}
