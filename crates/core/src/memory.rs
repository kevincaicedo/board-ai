//! Memory candidates and what the policy decided about them (PRODUCT §7.4, LLD §2.8).

use serde::{Deserialize, Serialize};

/// What kind of thing a memory is. The policy is one exhaustive `match` on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    SessionRecord,
    LearningSignal,
    /// What you said about yourself or your systems.
    Stated,
    /// What BoardAI concluded.
    Inferred,
    Note,
    Task,
    RawAudio,
    Secret,
}

/// What the policy does with a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAction {
    /// Kept automatically.
    Keep,
    /// Listed in the digest; kept only after a yes.
    Propose,
    /// Kept because you asked.
    OnRequest,
    /// Never written.
    Never,
}

/// A memory on its way to the store. `action` is never `Never`: those are not emitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWrite {
    pub kind: MemoryKind,
    pub action: MemoryAction,
    pub text: String,
    /// Session, element, project or `path:line` links.
    pub links: Vec<String>,
    /// The project a task belongs to.
    pub project: Option<String>,
}
