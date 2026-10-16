//! Memory candidates and what the policy decided about them (PRODUCT §7.4, LLD §2.8).

use serde::{Deserialize, Serialize};

use crate::ids::CaptureId;

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

/// Who a memory is about. It decides the memory file: `memory/about-you.md` or
/// `memory/facts.md` (LLD §3.6). The talker says it; the store never guesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// The user: preferences, how they learn, what they covered.
    You,
    /// The user's systems: their services, stack and setup.
    Systems,
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
    /// Who it is about, for stated and inferred memories. Learning signals are always about
    /// you. Notes, tasks and session records are filed by kind, so they carry `You` and the
    /// store does not read it.
    pub subject: Subject,
    /// The core's handle for this capture.
    pub capture: CaptureId,
}
