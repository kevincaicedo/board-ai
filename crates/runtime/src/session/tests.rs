//! The orchestrator end to end, on tokio's paused clock: a scripted talker, the virtual voice
//! (or a voice that refuses), the bridge, and recording sinks. No network, keys or audio.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::effect::{ForgetScope, Gain, LogRecord, Milestone};
use boardai_core::history::{Conversation, ProviderFamily, RawAssistant};
use boardai_core::ids::{CaptureId, ClauseId};
use boardai_core::input::{Input, StopReason};
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::{OrbState, UiBatch, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use boardai_core::{CoreConfig, MicMode};
use futures_util::future::BoxFuture;
use futures_util::{StreamExt, stream};

use super::ports::{SessionClose, SessionOpened, SpeechOut, SpeechOutError, StoreRead, StoreSink};
use super::{
    MemoryStore, SessionChannels, SessionHandle, SessionParts, SessionStats, VirtualVoice,
    VirtualVoiceConfig, spawn,
};
use crate::bridge::{UiSink, UiSinkClosed};
use crate::cancel::CancelToken;
use crate::clock::TokioClock;
use crate::llm::{ChatEvent, ChatStream, LlmCapabilities, LlmProvider, Usage};
use crate::tools::RepoTools;

const DEMO: &str = concat!(
    r#"<plan steps="Client|Edge"/>Sure. It starts in the "#,
    r#"<node id="client" label="Browser" kind="actor"/>browser, then hits the "#,
    r#"<node id="cdn" label="CDN" near="client"/><edge from="client" to="cdn"/>CDN at the edge."#,
);

/// One scripted reply: events with the delay before each.
type Reply = Vec<(u64, ChatEvent)>;

/// Counts streams dropped, so tests can see a cancelled stream really went away.
struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ScriptedTalker {
    /// `None` is a stream that never ends, like a model still thinking.
    replies: Mutex<VecDeque<Option<Reply>>>,
    requests: Mutex<Vec<Conversation>>,
    dropped: Arc<AtomicUsize>,
}

impl ScriptedTalker {
    fn says(&self, text: &str) {
        let chunks: Vec<(u64, ChatEvent)> = text
            .as_bytes()
            .chunks(24)
            .map(|chunk| {
                (
                    30,
                    ChatEvent::TextDelta(String::from_utf8_lossy(chunk).into()),
                )
            })
            .collect();
        self.push(chunks, StopReason::EndTurn, text);
    }

    fn push(&self, mut events: Reply, stop: StopReason, text: &str) {
        let raw = RawAssistant {
            provider: ProviderFamily::OpenAiCompatible,
            content_json: serde_json::json!({ "content": text }).to_string(),
        };
        let usage = Usage::default();
        let raw = Some(raw);
        events.push((10, ChatEvent::Done { stop, usage, raw }));
        if let Ok(mut replies) = self.replies.lock() {
            replies.push_back(Some(events));
        }
    }

    fn pending(&self) {
        if let Ok(mut replies) = self.replies.lock() {
            replies.push_back(None);
        }
    }

    fn requests(&self) -> Vec<Conversation> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }
}

impl LlmProvider for ScriptedTalker {
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            mid_conversation_system: true,
            tools: true,
            server_side_fallback: false,
        }
    }

    fn stream(&self, conversation: &Conversation, _cancel: CancelToken) -> ChatStream {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(conversation.clone());
        }
        let guard = DropCount(Arc::clone(&self.dropped));
        let reply = self
            .replies
            .lock()
            .ok()
            .and_then(|mut replies| replies.pop_front())
            .flatten();
        let Some(reply) = reply else {
            return Box::pin(stream::pending().map(move |item| {
                let _keep = &guard;
                item
            }));
        };
        Box::pin(stream::iter(reply).then(move |(delay_ms, event)| {
            let _keep = &guard;
            async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(event)
            }
        }))
    }
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<(SessionTime, UiEvent)>>,
    inputs: Mutex<Vec<Input>>,
    marks: Mutex<Vec<Milestone>>,
    flushed: Mutex<bool>,
    memory: MemoryStore,
    /// Sessions opened, and how each one ended (`discard` or `digest`), in order.
    lifecycle: Mutex<Vec<&'static str>>,
}

impl Recorder {
    fn events(&self) -> Vec<UiEvent> {
        let events = self
            .events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();
        events.into_iter().map(|(_, event)| event).collect()
    }

    fn inputs(&self) -> Vec<Input> {
        self.inputs
            .lock()
            .map(|inputs| inputs.clone())
            .unwrap_or_default()
    }

    fn marks(&self) -> Vec<Milestone> {
        self.marks
            .lock()
            .map(|marks| marks.clone())
            .unwrap_or_default()
    }

    fn note(&self, what: &'static str) {
        if let Ok(mut lifecycle) = self.lifecycle.lock() {
            lifecycle.push(what);
        }
    }

    fn lifecycle(&self) -> Vec<&'static str> {
        self.lifecycle
            .lock()
            .map(|lifecycle| lifecycle.clone())
            .unwrap_or_default()
    }

    fn orb(&self) -> Option<OrbState> {
        self.events().iter().rev().find_map(|event| {
            if let UiEvent::Orb { state, .. } = event {
                Some(*state)
            } else {
                None
            }
        })
    }
}

impl UiSink for Recorder {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed> {
        if let Ok(mut events) = self.events.lock() {
            events.extend(batch.events.into_iter().map(|event| (batch.at, event)));
        }
        Ok(())
    }
}

impl StoreSink for Recorder {
    fn input(&self, _at: SessionTime, input: &Input) {
        if let Ok(mut inputs) = self.inputs.lock() {
            inputs.push(input.clone());
        }
    }

    fn ui_event(&self, _at: SessionTime, _event: &UiEvent) {}

    fn log(&self, _at: SessionTime, record: &LogRecord) {
        if let (LogRecord::Milestone { milestone }, Ok(mut marks)) = (record, self.marks.lock()) {
            marks.push(*milestone);
        }
    }

    fn remember(&self, at: SessionTime, memory: &MemoryWrite) {
        self.memory.remember(at, memory);
    }

    fn forget(&self, at: SessionTime, scope: ForgetScope) {
        self.memory.forget(at, scope);
    }

    fn accept_proposal(&self, at: SessionTime, capture: CaptureId) {
        self.memory.accept_proposal(at, capture);
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        self.memory.read(read)
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        self.note("start");
        Box::pin(std::future::ready(SessionOpened {
            session: Some("2026-10-15-143210".to_owned()),
            context: Some("Where you left off: Raft.".to_owned()),
            suggestion: Some("Pick up Raft where we left off?".to_owned()),
        }))
    }

    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()> {
        self.note(match close {
            SessionClose::Discard => "discard",
            SessionClose::Digest { .. } => "digest",
        });
        Box::pin(std::future::ready(()))
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        if let Ok(mut flushed) = self.flushed.lock() {
            *flushed = true;
        }
        Box::pin(std::future::ready(()))
    }
}

/// A speech out whose actor is gone: it refuses everything.
struct RefusingVoice;

impl SpeechOut for RefusingVoice {
    fn synthesize(&mut self, _clause: ClauseId, _text: String) -> Result<(), SpeechOutError> {
        Err(SpeechOutError::Closed)
    }

    fn duck(&mut self, _gain: Gain) {}

    fn stop(&mut self) -> Result<(), SpeechOutError> {
        Err(SpeechOutError::Closed)
    }

    fn reset(&mut self) {}
}

struct Harness {
    session: SessionHandle,
    talker: Arc<ScriptedTalker>,
    recorder: Arc<Recorder>,
}

fn start(talker: Arc<ScriptedTalker>, refusing_voice: bool) -> Harness {
    let channels = SessionChannels::new();
    let speech: Box<dyn SpeechOut> = if refusing_voice {
        Box::new(RefusingVoice)
    } else {
        Box::new(VirtualVoice::spawn(
            channels.speech_out(),
            VirtualVoiceConfig::default(),
        ))
    };
    let recorder = Arc::new(Recorder::default());
    let parts = SessionParts {
        core: CoreConfig {
            talker_name: "the model".to_owned(),
            mic: MicMode::OpenMic,
            session_context: None,
            suggestion: None,
        },
        clock: TokioClock::start(),
        talker: Arc::clone(&talker) as Arc<dyn LlmProvider>,
        tools: RepoTools::default(),
        harness: None,
        speech,
        store: Arc::clone(&recorder) as Arc<dyn StoreSink>,
        ui: Arc::clone(&recorder) as Arc<dyn UiSink>,
    };
    Harness {
        session: spawn(channels, parts),
        talker,
        recorder,
    }
}

/// Lets the session run until `done` holds, checking every 50 ms of session time.
async fn run_until(limit_ms: u64, done: impl Fn() -> bool) -> bool {
    for _ in 0..limit_ms / 50 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    done()
}

fn idle_after_speaking(recorder: &Recorder) -> bool {
    let spoke = recorder.events().iter().any(|event| {
        matches!(
            event,
            UiEvent::Orb {
                state: OrbState::Drawing | OrbState::Speaking,
                ..
            }
        )
    });
    spoke && recorder.orb() == Some(OrbState::Idle)
}

async fn close(harness: Harness) -> (SessionStats, Arc<Recorder>, Arc<ScriptedTalker>) {
    let stats = harness.session.close().await;
    (stats, harness.recorder, harness.talker)
}

mod flow;
mod lifecycle;
