//! The session digest (PRODUCT §7.4, LLD §3.6): written at session end by the scribe, or as a
//! minimal digest when the scribe fails. `digest.md` has stable sections the index parses back,
//! so the graph and pending proposals survive an index rebuild.

mod document;
mod scribe;

use serde::{Deserialize, Serialize};

pub(crate) use document::{DigestDocument, ParsedDigest, ProposalLine, parse_digest};
pub use scribe::{
    SCRIBE_SYSTEM_PROMPT, SessionMaterial, TranscriptLine, digest_with_scribe, fallback_digest,
    scribe_config,
};

/// What the digest says about a session, before the store adds what it knows (ids, captured
/// items, the board link).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    pub title: String,
    pub summary: String,
    /// Names of the key diagrams, linked to `board.excalidraw`.
    pub diagrams: Vec<String>,
    pub open_questions: Vec<String>,
    /// Memories the scribe inferred. They are proposals: kept only after a yes.
    pub proposals: Vec<ProposalDraft>,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub written_by: WrittenBy,
}

/// Who wrote the digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum WrittenBy {
    Scribe,
    /// The minimal digest, and why the scribe's wasn't used.
    Fallback {
        reason: FallbackReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FallbackReason {
    /// No scribe is configured.
    NoScribe,
    /// The provider failed; `detail` keeps its category.
    ProviderFailed {
        detail: String,
    },
    /// The answer did not end normally (token limit, refusal).
    Incomplete,
    /// The answer was not the JSON shape asked for.
    BadOutput {
        detail: String,
    },
    TimedOut,
    Cancelled,
    /// No digest was asked for before the app quit or the next session started.
    Interrupted,
}

/// A memory waiting for a yes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalDraft {
    pub about: About,
    pub text: String,
}

/// Which memory file a memory belongs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum About {
    /// `memory/about-you.md`: preferences and how you learn.
    You,
    /// `memory/facts.md`: your systems.
    Systems,
}

impl About {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Systems => "systems",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "you" => Some(Self::You),
            "systems" => Some(Self::Systems),
            _ => None,
        }
    }
}

/// A node of the knowledge graph (PRODUCT §7.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub kind: EntityKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Concept,
    System,
    File,
    Project,
}

impl EntityKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Concept => "concept",
            Self::System => "system",
            Self::File => "file",
            Self::Project => "project",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        [Self::Concept, Self::System, Self::File, Self::Project]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// An edge of the knowledge graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub subject: String,
    pub relation: RelationKind,
    pub object: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Explains,
    PartOf,
    DependsOn,
    MentionedIn,
    StruggledWith,
}

impl RelationKind {
    pub(crate) const ALL: [Self; 5] = [
        Self::Explains,
        Self::PartOf,
        Self::DependsOn,
        Self::MentionedIn,
        Self::StruggledWith,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Explains => "explains",
            Self::PartOf => "part_of",
            Self::DependsOn => "depends_on",
            Self::MentionedIn => "mentioned_in",
            Self::StruggledWith => "struggled_with",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}
