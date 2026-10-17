//! Inputs are written to the session event log and read back for re-simulation, so every
//! variant must survive a JSON round trip. Internally tagged enums cannot carry bare primitives;
//! this catches a variant that would only fail at runtime.
#![allow(
    clippy::expect_used,
    reason = "a broken fixture should fail the test loudly"
)]

use boardai_core::effect::{Effect, ForgetScope, Gain, LogRecord, Milestone, TalkRequest};
use boardai_core::geometry::Rect;
use boardai_core::harness::{HarnessEvent, HarnessKind, HarnessTask};
use boardai_core::history::{AssistantPart, Message, ProviderFamily, RawAssistant, UserPart};
use boardai_core::ids::{CaptureId, ClauseId, ElementId, LayerId, OpId, TaskId, ToolCallId};
use boardai_core::input::{
    Input, PlaybackPosition, StopReason, TalkerFailure, TalkerFailureKind, TimingSource,
    WordTiming, WordTimings,
};
use boardai_core::memory::{MemoryAction, MemoryChange, MemoryKind, MemoryWrite, Subject};
use boardai_core::protocol::{DrawOp, MicAction, NodeShape, PointerTarget, UiCommand, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::tools::{ToolCall, ToolInvocation, ToolOutput};
use boardai_core::views::{
    CoverShape, DigestCard, MemoryRow, NoteRow, PastLayer, SessionCard, SessionRef, TaskRow, View,
};

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(&back, value, "{json}");
}

fn tool_id() -> ToolCallId {
    ToolCallId::parse("toolu_01").expect("valid")
}

#[test]
fn every_input_round_trips() {
    for input in speech_and_command_inputs()
        .iter()
        .chain(&talker_and_tool_inputs())
    {
        round_trip(input);
    }
}

fn speech_and_command_inputs() -> Vec<Input> {
    let position = PlaybackPosition {
        clause: ClauseId(2),
        offset_ms: 480,
    };
    let timings = WordTimings {
        source: TimingSource::Estimated,
        words: vec![WordTiming {
            byte_offset: 0,
            start_ms: 0,
        }],
        duration_ms: 900,
    };
    let pointer = PointerTarget::Element {
        layer: LayerId::MAIN,
        id: ElementId::parse("lb").expect("valid"),
    };
    vec![
        Input::VoiceOnset,
        Input::VoiceStart,
        Input::VoiceEnd {
            voiced_ms: 640,
            silence_ms: 256,
        },
        Input::Transcript {
            text: "wait, why".into(),
            is_final: true,
        },
        Input::ClauseReady {
            clause: ClauseId(2),
            timings,
        },
        Input::SynthesisFailed {
            clause: ClauseId(3),
        },
        Input::PlaybackAt { position },
        Input::PlaybackDrained,
        Input::VoiceStopped {
            last: Some(position),
        },
        Input::Command(UiCommand::Pointer {
            target: Some(pointer),
        }),
        Input::Command(UiCommand::Mic {
            action: MicAction::Mute,
        }),
        Input::Command(UiCommand::CloseView),
        Input::Tick,
        Input::MemoryFailed {
            change: MemoryChange::Remember {
                capture: CaptureId(3),
            },
            detail: "memory/facts.md: permission denied".into(),
        },
        Input::MemoryFailed {
            change: MemoryChange::ForgetSession,
            detail: "the store has stopped".into(),
        },
    ]
}

fn card() -> SessionCard {
    SessionCard {
        id: "2026-10-12-093000".into(),
        title: "Raft · log compaction".into(),
        date: "2026-10-12".into(),
        duration_ms: 1_860_000,
        cover: vec![CoverShape {
            rect: Rect {
                x: 120,
                y: 250,
                w: 150,
                h: 64,
            },
            shape: NodeShape::Service,
        }],
    }
}

/// Every view kind: they travel in tool results (logged as inputs) and in UI events.
fn views() -> Vec<View> {
    let from = SessionRef {
        id: "2026-10-14-180000".into(),
        title: "Request path · API".into(),
        date: "2026-10-14".into(),
    };
    let note = NoteRow {
        id: "n4".into(),
        text: "Cache-aside: Redis first.".into(),
        date: "2026-10-14".into(),
        from: Some(from.clone()),
        links: vec!["service.go:43".into()],
    };
    let leader = DrawOp::Node {
        id: ElementId::parse("leader").expect("valid"),
        op: OpId(0),
        shape: NodeShape::Service,
        label: "leader".into(),
        rect: Rect {
            x: 120,
            y: 250,
            w: 150,
            h: 64,
        },
        stack: 1,
        fill: None,
        link: None,
    };
    vec![
        View::Library {
            sessions: vec![card()],
            more_sessions: 2,
        },
        View::Tasks {
            project: Some("Harness".into()),
            tasks: vec![TaskRow {
                id: "t27".into(),
                title: "Check Redis TTLs".into(),
                project: "Harness".into(),
                from: Some(from),
            }],
            more_tasks: 0,
            notes: vec![note.clone()],
        },
        View::Memory {
            about_you: vec![MemoryRow {
                id: "m2".into(),
                text: "Examples in Go".into(),
            }],
            systems: Vec::new(),
            notes: vec![note],
            latest: Some(DigestCard {
                session: card(),
                notes: 2,
                tasks: 1,
                open_questions: 1,
            }),
        },
        View::PastSession {
            session: card(),
            recap: "A snapshot replaces the log.".into(),
            layers: vec![PastLayer {
                title: None,
                ops: vec![leader],
            }],
            ops_left_out: 0,
        },
        View::Replay { session: card() },
    ]
}

#[test]
fn every_view_round_trips_in_events_and_tool_results() {
    for view in views() {
        round_trip(&UiEvent::ViewShow {
            view: view.clone(),
            today: "2026-10-15".into(),
        });
        round_trip(&Input::ToolResult {
            id: tool_id(),
            output: ToolOutput::View {
                view: Box::new(view),
                today: "2026-10-15".into(),
            },
        });
    }
    round_trip(&UiEvent::ViewDismiss);
    round_trip(&UiEvent::OpenSettings);
    round_trip(&UiEvent::PageTurn {
        page: 2,
        title: Some("DNS".into()),
    });
    round_trip(&UiEvent::SessionStart {
        session: Some("2026-10-15-143210".into()),
    });
}

fn talker_and_tool_inputs() -> Vec<Input> {
    let failure = TalkerFailure {
        kind: TalkerFailureKind::RateLimited {
            retry_after_ms: Some(2000),
        },
        detail: "429".into(),
    };
    vec![
        Input::TalkerDelta {
            text: "Then <node id=\"lb\"/>it".into(),
        },
        Input::TalkerToolCall {
            invocation: ToolInvocation {
                id: tool_id(),
                name: "recall".into(),
                input_json: r#"{"query":"raft"}"#.into(),
                call: ToolCall::parse("recall", r#"{"query":"raft"}"#),
            },
        },
        Input::TalkerDone {
            stop: StopReason::Failed { failure },
            raw: Some(RawAssistant {
                provider: ProviderFamily::Anthropic,
                content_json: "[]".into(),
            }),
        },
        Input::ToolResult {
            id: tool_id(),
            output: ToolOutput::Text { text: "ok".into() },
        },
        Input::Harness {
            task: TaskId(1),
            event: HarnessEvent::Finished {
                summary: "done".into(),
                citations: vec![],
            },
        },
    ]
}

#[test]
fn every_effect_round_trips() {
    for effect in &effects() {
        round_trip(effect);
    }
}

fn effects() -> Vec<Effect> {
    let messages = vec![
        Message::System {
            text: "board: empty".into(),
        },
        Message::User {
            parts: vec![UserPart::Text { text: "hi".into() }],
        },
        Message::Assistant {
            parts: vec![AssistantPart::ToolUse {
                id: tool_id(),
                name: "recall".into(),
                input_json: "{}".into(),
            }],
            raw: None,
        },
    ];
    let change = HarnessTask {
        kind: HarnessKind::Change {
            branch: "boardai/retry".into(),
        },
        brief: "add jitter".into(),
    };
    let memory = MemoryWrite {
        kind: MemoryKind::Stated,
        action: MemoryAction::Keep,
        text: "we use Kafka".into(),
        links: vec![],
        project: None,
        subject: Subject::Systems,
        capture: CaptureId(4),
    };
    vec![
        Effect::Duck { gain: Gain::DUCKED },
        Effect::StopVoice,
        Effect::Synthesize {
            clause: ClauseId(1),
            text: "load balancer".into(),
        },
        Effect::Talk {
            request: TalkRequest { messages },
        },
        Effect::CancelTalk,
        Effect::RunTool {
            id: tool_id(),
            call: ToolCall::Recall {
                query: "raft".into(),
            },
        },
        Effect::RunHarness {
            task: TaskId(3),
            spec: change,
        },
        Effect::CancelHarness { task: TaskId(3) },
        Effect::Ui(UiEvent::Acknowledge),
        Effect::Log(LogRecord::Milestone {
            milestone: Milestone::FirstOp,
        }),
        Effect::Remember(memory),
        Effect::WakeAt {
            at: SessionTime::from_ms(1500),
        },
    ]
}

#[test]
fn every_memory_effect_round_trips() {
    for effect in [
        Effect::Forget {
            scope: ForgetScope::Last,
        },
        Effect::Forget {
            scope: ForgetScope::Session,
        },
        Effect::AcceptProposal {
            capture: CaptureId(4),
        },
    ] {
        round_trip(&effect);
    }
}
