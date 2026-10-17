//! The task that owns the realtime connection: one WebSocket per voice, opened on demand (or by
//! a warm-up), kept across replies, and closed after [`ELEVENLABS_IDLE_CLOSE_MS`] without a
//! clause, a little before the server would close it.
//!
//! Each clause is one context: its text, a flush and a close go out at once, and the clause is
//! answered when the server's final message for the context arrives, with all its audio and the
//! alignment as word timings. The whole clause is answered at once because speech out needs a
//! clause's complete timings (and its length) before its first frame may play.
//!
//! Every way the connection can fail answers each waiting clause exactly once:
//!
//! - `Unavailable`: the WebSocket could not carry it (no connection, a dropped socket, a
//!   protocol error); the engine retries the clause over HTTP. A failure to connect or a protocol
//!   error also rests the WebSocket for [`ELEVENLABS_REALTIME_REST_MS`].
//! - `Failed`: the server refused it (a bad key, a quota, an error message) or it stalled past
//!   [`ELEVENLABS_CONTEXT_IDLE_TIMEOUT_MS`]; the engine reports it and the fallback voice speaks.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::input::WordTiming;
use reqwest::header::HeaderMap;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::wire::{self, ContextId, ServerMessage};
use crate::audio::VOICE_RATE_HZ;
use crate::limits::{
    ELEVENLABS_CLOSE_TIMEOUT_MS, ELEVENLABS_COMMAND_QUEUE, ELEVENLABS_CONNECT_TIMEOUT_MS,
    ELEVENLABS_CONTEXT_IDLE_TIMEOUT_MS, ELEVENLABS_CONTEXTS_MAX, ELEVENLABS_IDLE_CLOSE_MS,
    ELEVENLABS_LINE_MAX_BYTES, ELEVENLABS_REALTIME_REST_MS, ELEVENLABS_RESPONSE_MAX_BYTES,
    ELEVENLABS_SEND_TIMEOUT_MS,
};
use crate::speech_out::TtsError;
use crate::speech_out::elevenlabs::alignment::Alignment;
use crate::speech_out::elevenlabs::{from_provider, pcm_from_base64};
use crate::speech_out::websocket::{
    self, CLOSE_NORMAL, CLOSE_PROTOCOL_ERROR, Incoming, WebSocket, WebSocketError,
};

/// Where and how to connect; shared by the engine and its task.
pub(crate) struct Endpoint {
    pub(crate) client: reqwest::Client,
    pub(crate) headers: HeaderMap,
    pub(crate) base_url: String,
    pub(crate) model: String,
}

impl Endpoint {
    /// The handshake URL for a voice id already checked for a path.
    fn url(&self, voice: &str) -> Result<String, TtsError> {
        super::handshake_url(&self.base_url, voice, &self.model)
    }
}

pub(crate) enum Command {
    Speak(Speak),
    /// The clause's stream was dropped (a stop): forget its context.
    Cancel(ContextId),
    /// Connect now, so the first clause does not pay for the handshake.
    WarmUp {
        voice: String,
    },
}

pub(crate) struct Speak {
    pub(crate) context: ContextId,
    pub(crate) voice: String,
    pub(crate) text: String,
    pub(crate) reply: oneshot::Sender<Outcome>,
}

/// How a clause's context ended.
#[derive(Debug)]
pub(crate) enum Outcome {
    Complete {
        audio: Vec<f32>,
        /// `None` when the alignment did not spell the clause: speech out estimates.
        words: Option<Vec<WordTiming>>,
        timing: ContextTiming,
    },
    /// The WebSocket could not carry the clause; HTTP may.
    Unavailable { error: TtsError },
    /// The provider refused the clause, or it stalled.
    Failed { error: TtsError },
}

/// When a clause's messages arrived, from the moment its text was sent.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContextTiming {
    /// The connection was opened for this clause (the handshake is not included).
    pub(crate) fresh_connection: bool,
    pub(crate) first_audio: Option<Duration>,
    pub(crate) last_audio: Option<Duration>,
    pub(crate) finished: Duration,
    pub(crate) messages: usize,
}

/// Starts the task on `runtime`. It ends when every sender is gone.
pub(crate) fn spawn(
    runtime: &tokio::runtime::Handle,
    endpoint: Arc<Endpoint>,
    recorder: Option<std::sync::mpsc::Sender<String>>,
) -> mpsc::Sender<Command> {
    let (commands, receiver) = mpsc::channel(ELEVENLABS_COMMAND_QUEUE);
    let connection = Connection {
        endpoint,
        socket: None,
        contexts: BTreeMap::new(),
        rest_until: None,
        recorder,
    };
    runtime.spawn(connection.run(receiver));
    commands
}

struct Open {
    socket: WebSocket<reqwest::Upgraded>,
    voice: String,
    last_activity: Instant,
}

struct Context {
    text: String,
    reply: oneshot::Sender<Outcome>,
    audio: Vec<f32>,
    alignment: Alignment,
    received_bytes: usize,
    deadline: Instant,
    sent_at: Instant,
    first_audio: Option<Instant>,
    last_audio: Option<Instant>,
    messages: usize,
    fresh_connection: bool,
}

/// How the contexts still waiting are answered when the connection goes away.
#[derive(Clone)]
enum Ending {
    Unavailable(TtsError),
    Failed(TtsError),
}

impl Ending {
    const fn error(&self) -> &TtsError {
        match self {
            Self::Unavailable(error) | Self::Failed(error) => error,
        }
    }

    fn outcome(&self) -> Outcome {
        match self {
            Self::Unavailable(error) => Outcome::Unavailable {
                error: error.clone(),
            },
            Self::Failed(error) => Outcome::Failed {
                error: error.clone(),
            },
        }
    }
}

/// How a connection ends: we close it, it is already broken or closing (only our close frame
/// goes out), or the server broke the protocol.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Ours,
    Brief,
    ProtocolError,
}

enum Next {
    Command(Option<Command>),
    Message(Result<Incoming, WebSocketError>),
    Deadline,
}

struct Connection {
    endpoint: Arc<Endpoint>,
    socket: Option<Open>,
    contexts: BTreeMap<ContextId, Context>,
    rest_until: Option<Instant>,
    /// Every text message received, for the live check's record of the wire format.
    recorder: Option<std::sync::mpsc::Sender<String>>,
}

impl Connection {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        loop {
            let deadline = self.next_deadline();
            let next = tokio::select! {
                command = commands.recv() => Next::Command(command),
                message = next_message(&mut self.socket) => Next::Message(message),
                () = sleep_until(deadline) => Next::Deadline,
            };
            match next {
                Next::Command(None) => break,
                Next::Command(Some(command)) => self.on_command(command).await,
                Next::Message(message) => self.on_message(message).await,
                Next::Deadline => self.on_deadline().await,
            }
        }
        let ending = Ending::Unavailable(TtsError::Unavailable {
            detail: "the realtime voice stopped".to_owned(),
        });
        self.close(&ending, Closing::Ours).await;
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::Speak(speak) => self.speak(speak).await,
            Command::Cancel(context) => {
                // Its close went out with its text; whatever still arrives for it is ignored.
                self.contexts.remove(&context);
            }
            Command::WarmUp { voice } => {
                if self.resting(Instant::now()).is_none()
                    && let Err(ending) = self.ensure_open(&voice).await
                {
                    let error = ending.error();
                    tracing::warn!(%error, "the realtime voice could not connect ahead");
                }
            }
        }
    }

    async fn speak(&mut self, speak: Speak) {
        let Speak {
            context,
            voice,
            text,
            reply,
        } = speak;
        if reply.is_closed() {
            return;
        }
        if let Some(error) = self.resting(Instant::now()) {
            let _ = reply.send(Outcome::Unavailable { error });
            return;
        }
        if self.contexts.len() >= ELEVENLABS_CONTEXTS_MAX {
            let error = TtsError::Unavailable {
                detail: "too many realtime clauses at once".to_owned(),
            };
            let _ = reply.send(Outcome::Unavailable { error });
            return;
        }
        let fresh_connection = match self.ensure_open(&voice).await {
            Ok(fresh) => fresh,
            Err(ending) => {
                let _ = reply.send(ending.outcome());
                return;
            }
        };
        if reply.is_closed() {
            // Stopped while connecting: its text is never sent, so never billed.
            return;
        }
        let messages = [
            wire::context_text(context, &text),
            wire::flush(context),
            wire::close_context(context),
        ];
        for message in &messages {
            if let Err(error) = self.send(message).await {
                let ending = Ending::Unavailable(error);
                let _ = reply.send(ending.outcome());
                self.close(&ending, Closing::Brief).await;
                return;
            }
        }
        let now = Instant::now();
        let state = Context {
            text,
            reply,
            audio: Vec::new(),
            alignment: Alignment::default(),
            received_bytes: 0,
            deadline: now + Duration::from_millis(ELEVENLABS_CONTEXT_IDLE_TIMEOUT_MS),
            sent_at: now,
            first_audio: None,
            last_audio: None,
            messages: 0,
            fresh_connection,
        };
        self.contexts.insert(context, state);
    }

    /// Why clauses go over HTTP right now, if the WebSocket is resting.
    fn resting(&mut self, now: Instant) -> Option<TtsError> {
        let until = self.rest_until?;
        if now >= until {
            self.rest_until = None;
            return None;
        }
        Some(TtsError::Unavailable {
            detail: "the realtime voice is resting after a failure".to_owned(),
        })
    }

    /// Opens the connection for `voice` unless it is open and fresh. `Ok(true)` when it was
    /// opened now.
    async fn ensure_open(&mut self, voice: &str) -> Result<bool, Ending> {
        let now = Instant::now();
        if let Some(open) = &self.socket {
            let stale = now >= open.last_activity + Duration::from_millis(ELEVENLABS_IDLE_CLOSE_MS);
            if open.voice == voice && !stale {
                return Ok(false);
            }
            let ending = Ending::Unavailable(TtsError::Unavailable {
                detail: "the realtime connection was replaced".to_owned(),
            });
            self.close(&ending, Closing::Ours).await;
        }
        let url = self.endpoint.url(voice).map_err(Ending::Failed)?;
        let connect = websocket::connect(
            &self.endpoint.client,
            &url,
            self.endpoint.headers.clone(),
            ELEVENLABS_LINE_MAX_BYTES,
        );
        let timeout = Duration::from_millis(ELEVENLABS_CONNECT_TIMEOUT_MS);
        let error = match tokio::time::timeout(timeout, connect).await {
            Ok(Ok(socket)) => {
                self.socket = Some(Open {
                    socket,
                    voice: voice.to_owned(),
                    last_activity: Instant::now(),
                });
                return Ok(true);
            }
            Ok(Err(error)) => from_provider(error),
            Err(_elapsed) => TtsError::Timeout,
        };
        Err(self.connect_failed(error))
    }

    /// A refused key or a quota would refuse HTTP too; anything else may be the WebSocket alone.
    fn connect_failed(&mut self, error: TtsError) -> Ending {
        match error {
            TtsError::Auth { .. } | TtsError::RateLimited { .. } => Ending::Failed(error),
            TtsError::Network { .. }
            | TtsError::Timeout
            | TtsError::BadOutput { .. }
            | TtsError::Provider { .. }
            | TtsError::Unavailable { .. } => {
                self.rest();
                Ending::Unavailable(error)
            }
        }
    }

    fn rest(&mut self) {
        self.rest_until = Some(Instant::now() + Duration::from_millis(ELEVENLABS_REALTIME_REST_MS));
    }

    async fn send(&mut self, message: &str) -> Result<(), TtsError> {
        let Some(open) = self.socket.as_mut() else {
            return Err(TtsError::Network {
                detail: "the realtime connection is closed".to_owned(),
            });
        };
        let timeout = Duration::from_millis(ELEVENLABS_SEND_TIMEOUT_MS);
        match tokio::time::timeout(timeout, open.socket.send_text(message)).await {
            Ok(Ok(())) => {
                open.last_activity = Instant::now();
                Ok(())
            }
            Ok(Err(error)) => Err(TtsError::Network {
                detail: error.to_string(),
            }),
            Err(_elapsed) => Err(TtsError::Timeout),
        }
    }

    async fn on_message(&mut self, message: Result<Incoming, WebSocketError>) {
        if let Some(open) = self.socket.as_mut() {
            open.last_activity = Instant::now();
        }
        match message {
            Ok(Incoming::Text(text)) => self.on_text(&text).await,
            Ok(Incoming::Ping(payload)) => {
                let pong = match self.socket.as_mut() {
                    Some(open) => open.socket.send_pong(&payload).await,
                    None => Ok(()),
                };
                if let Err(error) = pong {
                    let ending = Ending::Unavailable(TtsError::Network {
                        detail: error.to_string(),
                    });
                    self.close(&ending, Closing::Brief).await;
                }
            }
            Ok(Incoming::Pong | Incoming::Binary(_)) => {}
            Ok(Incoming::Close { code, reason }) => self.on_close(code, &reason).await,
            Err(error) => {
                let protocol = matches!(error, WebSocketError::Protocol(_));
                let detail = error.to_string();
                let ending = if protocol {
                    self.rest();
                    Ending::Unavailable(TtsError::BadOutput { detail })
                } else {
                    Ending::Unavailable(TtsError::Network { detail })
                };
                let closing = if protocol {
                    Closing::ProtocolError
                } else {
                    Closing::Brief
                };
                self.close(&ending, closing).await;
            }
        }
    }

    async fn on_text(&mut self, text: &str) {
        if let Some(recorder) = &self.recorder {
            let _ = recorder.send(text.to_owned());
        }
        match wire::parse(text) {
            Ok(ServerMessage::Chunk {
                context,
                audio,
                alignment,
                is_final,
            }) => {
                if let Some(context) = context {
                    self.on_chunk(context, audio.as_deref(), alignment, is_final);
                }
            }
            Ok(ServerMessage::Error {
                context: Some(context),
                error,
            }) => {
                if let Some(state) = self.contexts.remove(&context) {
                    let _ = state.reply.send(Outcome::Failed { error });
                }
            }
            Ok(ServerMessage::Error {
                context: None,
                error,
            }) => {
                tracing::warn!(%error, "the realtime voice refused");
                self.answer_all(&Ending::Failed(error));
            }
            Err(error) => {
                self.rest();
                self.close(&Ending::Unavailable(error), Closing::ProtocolError)
                    .await;
            }
        }
    }

    fn on_chunk(
        &mut self,
        context: ContextId,
        audio: Option<&str>,
        alignment: Vec<(String, f64)>,
        is_final: bool,
    ) {
        let Some(state) = self.contexts.get_mut(&context) else {
            return;
        };
        if state.reply.is_closed() {
            self.contexts.remove(&context);
            return;
        }
        let now = Instant::now();
        state.messages += 1;
        state.deadline = now + Duration::from_millis(ELEVENLABS_CONTEXT_IDLE_TIMEOUT_MS);
        if let Err(error) = state.append(audio, alignment, now) {
            if let Some(state) = self.contexts.remove(&context) {
                let _ = state.reply.send(Outcome::Failed { error });
            }
            return;
        }
        if is_final && let Some(state) = self.contexts.remove(&context) {
            state.finish(now);
        }
    }

    async fn on_close(&mut self, code: Option<u16>, reason: &str) {
        // 1008 (policy) is how the server refuses a key or a quota; anything else ends the
        // connection without refusing the clauses, so they may go over HTTP.
        let ending = match code {
            Some(1008) => Ending::Failed(wire::classify("", reason)),
            Some(_) | None => Ending::Unavailable(TtsError::Network {
                detail: format!(
                    "the realtime connection closed ({}): {}",
                    code.map_or_else(|| "no code".to_owned(), |code| code.to_string()),
                    crate::llm::bounded_detail(reason)
                ),
            }),
        };
        self.close(&ending, Closing::Brief).await;
    }

    async fn on_deadline(&mut self) {
        let now = Instant::now();
        let stalled: Vec<ContextId> = self
            .contexts
            .iter()
            .filter(|(_, state)| state.deadline <= now)
            .map(|(context, _)| *context)
            .collect();
        for context in &stalled {
            if let Some(state) = self.contexts.remove(context) {
                let _ = state.reply.send(Outcome::Failed {
                    error: TtsError::Timeout,
                });
            }
        }
        let idle = self.contexts.is_empty()
            && self.socket.as_ref().is_some_and(|open| {
                now >= open.last_activity + Duration::from_millis(ELEVENLABS_IDLE_CLOSE_MS)
            });
        let ending = Ending::Unavailable(TtsError::Timeout);
        if !stalled.is_empty() {
            // A connection that stalled one clause is not trusted with the next.
            self.close(&ending, Closing::Brief).await;
        } else if idle {
            self.close(&ending, Closing::Ours).await;
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let contexts = self.contexts.values().map(|state| state.deadline).min();
        let idle = self
            .socket
            .as_ref()
            .filter(|_| self.contexts.is_empty())
            .map(|open| open.last_activity + Duration::from_millis(ELEVENLABS_IDLE_CLOSE_MS));
        contexts.into_iter().chain(idle).min()
    }

    fn answer_all(&mut self, ending: &Ending) {
        for (_, state) in std::mem::take(&mut self.contexts) {
            let _ = state.reply.send(ending.outcome());
        }
    }

    /// Answers every waiting clause, then closes the socket within a bound: our own close asks
    /// the server to close its contexts and waits for its close frame; otherwise only our close
    /// frame goes out.
    async fn close(&mut self, ending: &Ending, closing: Closing) {
        self.answer_all(ending);
        let Some(mut open) = self.socket.take() else {
            return;
        };
        let handshake = async {
            match closing {
                Closing::Ours => {
                    let _ = open.socket.send_text(&wire::close_socket()).await;
                    let _ = open.socket.send_close(CLOSE_NORMAL).await;
                    while !matches!(
                        open.socket.next().await,
                        Ok(Incoming::Close { .. }) | Err(_)
                    ) {}
                }
                Closing::Brief => {
                    let _ = open.socket.send_close(CLOSE_NORMAL).await;
                }
                Closing::ProtocolError => {
                    let _ = open.socket.send_close(CLOSE_PROTOCOL_ERROR).await;
                }
            }
        };
        let timeout = Duration::from_millis(ELEVENLABS_CLOSE_TIMEOUT_MS);
        let _ = tokio::time::timeout(timeout, handshake).await;
    }
}

impl Context {
    fn append(
        &mut self,
        audio: Option<&str>,
        alignment: Vec<(String, f64)>,
        now: Instant,
    ) -> Result<(), TtsError> {
        let message_start_ms = self.audio.len() as f64 * 1_000.0 / f64::from(VOICE_RATE_HZ);
        if let Some(audio) = audio {
            self.received_bytes += audio.len();
            if self.received_bytes > ELEVENLABS_RESPONSE_MAX_BYTES {
                return Err(TtsError::BadOutput {
                    detail: "a clause's audio is larger than its limit".to_owned(),
                });
            }
            let samples = pcm_from_base64(audio)?;
            self.audio.extend_from_slice(&samples);
            self.first_audio.get_or_insert(now);
            self.last_audio = Some(now);
        }
        self.alignment.push_message(alignment, message_start_ms);
        Ok(())
    }

    fn finish(self, now: Instant) {
        let since = |at: Instant| at - self.sent_at;
        let timing = ContextTiming {
            fresh_connection: self.fresh_connection,
            first_audio: self.first_audio.map(since),
            last_audio: self.last_audio.map(since),
            finished: since(now),
            messages: self.messages,
        };
        let outcome = if self.audio.is_empty() {
            // Nothing to play: HTTP gets a chance before the fallback voice does.
            Outcome::Unavailable {
                error: TtsError::BadOutput {
                    detail: "the realtime voice finished a clause without audio".to_owned(),
                },
            }
        } else {
            let audio_ms = self.audio.len() as f64 * 1_000.0 / f64::from(VOICE_RATE_HZ);
            let words = self.alignment.word_timings(&self.text, audio_ms);
            Outcome::Complete {
                audio: self.audio,
                words,
                timing,
            }
        };
        let _ = self.reply.send(outcome);
    }
}

async fn next_message(socket: &mut Option<Open>) -> Result<Incoming, WebSocketError> {
    match socket {
        Some(open) => open.socket.next().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
