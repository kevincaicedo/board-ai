//! The conversation state machine's states and the rules it decides with (LLD §2.2).
//!
//! Detours are not states: they are a stack in the thread, and speaking inside one is still
//! `Speaking`. Transitions live in the engine, which owns everything a transition touches.

pub mod barge_in;
pub mod cues;
pub mod draw_cues;
pub mod stage_cues;
pub mod utterance;

use crate::protocol::OrbState;
use barge_in::BargeIn;
use utterance::Utterance;

/// Who has the floor. Each state carries only what it needs, so a stale deadline from another
/// state cannot fire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationState {
    Idle,
    /// The user is talking; the turn ends on silence, the grace period or the utterance cap.
    Listening(Utterance),
    /// A talker request is out and nothing is audible yet.
    Thinking,
    Speaking,
    /// The user started talking over BoardAI; the voice is ducked while the core decides.
    Ducked(BargeIn),
    Muted,
}

impl ConversationState {
    /// The orb for this state. Ducked shows Listening: BoardAI is already yielding. Speaking may
    /// show Drawing instead while a stroke is in flight (the engine decides that).
    #[must_use]
    pub fn orb(&self) -> OrbState {
        match self {
            Self::Idle => OrbState::Idle,
            Self::Listening(_) | Self::Ducked(_) => OrbState::Listening,
            Self::Thinking => OrbState::Thinking,
            Self::Speaking => OrbState::Speaking,
            Self::Muted => OrbState::Muted,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening(_) => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Ducked(_) => "ducked",
            Self::Muted => "muted",
        }
    }
}
