//! Effects: what the core asks the runtime to do (LLD §2.1). The runtime applies them in order.
//! The core never waits; anything slow is an effect now and an input later.

use serde::{Deserialize, Serialize};

use crate::harness::HarnessTask;
use crate::history::Message;
use crate::ids::{CaptureId, ClauseId, TaskId, ToolCallId};
use crate::memory::MemoryWrite;
use crate::protocol::UiEvent;
use crate::sources::SourceEffect;
use crate::time::SessionTime;
use crate::tools::ToolCall;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Set the voice's gain; `Gain::FULL` restores it after a backchannel.
    Duck {
        gain: Gain,
    },
    /// Fade the voice out (30 ms) and answer with `Input::VoiceStopped`.
    StopVoice,
    /// Synthesize a clause's speakable text, then answer with `Input::ClauseReady`, or with
    /// `Input::SynthesisFailed` when it can't.
    Synthesize {
        clause: ClauseId,
        text: String,
    },
    /// Start a talker stream. At most one is in flight (I5); a `CancelTalk` precedes a new one.
    Talk {
        request: TalkRequest,
    },
    CancelTalk,
    RunTool {
        id: ToolCallId,
        call: ToolCall,
    },
    RunHarness {
        task: TaskId,
        spec: HarnessTask,
    },
    CancelHarness {
        task: TaskId,
    },
    Ui(UiEvent),
    Log(LogRecord),
    /// Pass a memory to the store. The memory policy has already decided it.
    Remember(MemoryWrite),
    /// Send `Input::Tick` at this time (or right away if it has passed).
    WakeAt {
        at: SessionTime,
    },
    /// "Forget that" or "don't remember this session" (PRODUCT §7.4, I10). The core has
    /// already said so out loud; the store undoes it.
    Forget {
        scope: ForgetScope,
    },
    /// "Yes, keep it": keep a memory that was only proposed. Only the user's own next words
    /// cause this, never a tool result or repo text (the same rule as I9).
    AcceptProposal {
        capture: CaptureId,
    },
    /// Sources (D-184, D-185): resolve a spoken name, share or forget sources, load and keep a
    /// project's, ask for a brief, cache one, read its citations.
    Source {
        source: SourceEffect,
    },
}

/// What "forget" covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgetScope {
    /// The last thing this session captured (the last `Remember` or `AcceptProposal` not yet
    /// forgotten).
    Last,
    /// The whole session: nothing of it stays in the vault, and nothing more is written.
    Session,
}

/// The voice's gain, from 0 (silent) to 1 (full).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Gain(f32);

impl Gain {
    pub const FULL: Self = Self(1.0);
    /// The duck level while BoardAI decides whether you are interrupting (LLD §2.2).
    pub const DUCKED: Self = Self(0.25);

    /// Clamps into `[0, 1]`; NaN becomes silence.
    #[must_use]
    pub fn new(value: f32) -> Self {
        if value.is_nan() {
            Self(0.0)
        } else {
            Self(value.clamp(0.0, 1.0))
        }
    }

    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// One talker request: the whole append-only history. The system prompt and the tool set are
/// fixed per session and owned by the adapter, so they are not repeated here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TalkRequest {
    pub messages: Vec<Message>,
}

/// Structured records for the session log and the latency overlay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum LogRecord {
    Milestone {
        milestone: Milestone,
    },
    /// Something was dropped at a limit, as LLD §5 requires.
    Dropped {
        what: Dropped,
        count: u32,
    },
}

/// The per-turn latency marks (LLD §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Milestone {
    VoiceStart,
    FinalTranscript,
    TalkerRequested,
    TalkerFirstToken,
    FirstAudio,
    FirstOp,
    BargeInDuck,
    BargeInStop,
    /// The voice ducked on a voice onset, before `VoiceStart` confirmed speech.
    OnsetDuck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dropped {
    NarrationTag,
    Op,
    Detour,
    /// A shape the user drew that the core refused (out of range, too many points or shapes, a
    /// closed layer), or an undo with nothing of this drawing to undo.
    UserShape,
    /// A drawing command that means nothing now: Done or Esc while not drawing, or Done while
    /// muted.
    DrawCommand,
    /// A staged diagram's part dropped at a limit (D-187): past the block's size or part count,
    /// nested too deep, a category past the palette, an id already taken, or a part that would
    /// land on a shape the user drew.
    DiagramPart,
}

#[cfg(test)]
mod tests {
    use super::Gain;

    #[test]
    fn gain_is_clamped() {
        assert!((Gain::new(2.0).value() - 1.0).abs() < f32::EPSILON);
        assert!(Gain::new(-1.0).value().abs() < f32::EPSILON);
        assert!(Gain::new(f32::NAN).value().abs() < f32::EPSILON);
    }
}
