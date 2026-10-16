//! Inputs: everything the runtime tells the core (LLD §2.1). Each is stamped with the session
//! time of the step that carries it.

use serde::{Deserialize, Serialize};

use crate::harness::HarnessEvent;
use crate::history::RawAssistant;
use crate::ids::{ClauseId, TaskId, ToolCallId};
use crate::protocol::UiCommand;
use crate::tools::{ToolInvocation, ToolOutput};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum Input {
    /// Voice activity started (VAD onset, after the minimum-speech filter).
    VoiceStart,
    /// Voice activity ended; `voiced_ms` is the length of the voiced span.
    VoiceEnd {
        voiced_ms: u32,
    },
    /// A transcript of the current utterance. Partials replace each other; the final closes it.
    Transcript {
        text: String,
        is_final: bool,
    },
    /// Streamed narration text from the talker.
    TalkerDelta {
        text: String,
    },
    /// A tool call, already parsed at the provider boundary.
    TalkerToolCall {
        invocation: ToolInvocation,
    },
    /// The talker's stream ended. `raw` is the provider's full assistant content when the
    /// stream completed (absent when it failed).
    TalkerDone {
        stop: StopReason,
        raw: Option<RawAssistant>,
    },
    /// A clause was synthesized; playback can report positions in it from now on.
    ClauseReady {
        clause: ClauseId,
        timings: WordTimings,
    },
    /// Speech out could not synthesize this clause, so it will never play. It answers a
    /// `Synthesize` instead of `ClauseReady`, never after it. The clause is skipped: its ops are
    /// never drawn, its text is shown as a problem caption, and the reply goes on without it.
    SynthesisFailed {
        clause: ClauseId,
    },
    /// Where playback is, from the audio sample counter minus output latency. The runtime
    /// coalesces these to at most one per 20 ms.
    PlaybackAt {
        position: PlaybackPosition,
    },
    /// Every synthesized clause has played.
    PlaybackDrained,
    /// The answer to `StopVoice`: the last position actually played, or `None` when nothing was
    /// playing. This defines the heard prefix (LLD §2.7).
    VoiceStopped {
        last: Option<PlaybackPosition>,
    },
    ToolResult {
        id: ToolCallId,
        output: ToolOutput,
    },
    Harness {
        task: TaskId,
        event: HarnessEvent,
    },
    Command(UiCommand),
    /// A deadline the core asked for with `WakeAt` was reached.
    Tick,
    /// The voice detector's first window above its speech threshold: maybe speech, maybe a
    /// cough. It comes about 160 ms before `VoiceStart` can confirm speech, so the voice can duck
    /// within the barge-in budget; a `VoiceStart` follows only if the speech lasts (LLD §2.2).
    VoiceOnset,
}

/// A position in the voice: `offset_ms` into the audio of `clause`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackPosition {
    pub clause: ClauseId,
    pub offset_ms: u32,
}

/// When each word of a clause starts in its audio (LLD §2.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordTimings {
    pub source: TimingSource,
    /// Sorted by `byte_offset`, which indexes the clause's speakable text.
    pub words: Vec<WordTiming>,
    /// Length of the clause's audio.
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordTiming {
    pub byte_offset: u32,
    pub start_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingSource {
    /// From the provider's alignment or the model's durations.
    Exact,
    /// From the character rate, scaled to the real audio length.
    Estimated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stop", rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    /// The response hit its token limit; the core finishes the current sentence.
    MaxTokens,
    /// The model refused; the user hears a short line, never silence.
    Refusal,
    Failed {
        failure: TalkerFailure,
    },
}

/// A talker failure that keeps its category (AGENTS.md, errors are values).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TalkerFailure {
    pub kind: TalkerFailureKind,
    /// For the log and the caption; never contains a secret.
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TalkerFailureKind {
    Auth,
    RateLimited {
        retry_after_ms: Option<u32>,
    },
    Network,
    Timeout,
    /// The provider sent something we couldn't parse.
    BadOutput,
    /// The provider reported an error of its own (overloaded, invalid request…).
    Provider,
    /// No talker is configured.
    NotConfigured,
}
