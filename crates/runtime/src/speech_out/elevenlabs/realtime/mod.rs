//! The ElevenLabs realtime engine: clauses over the multi-context WebSocket, with the HTTP
//! engine behind it for a clause the WebSocket cannot carry (see `connection` for which).
//!
//! The connection task starts on the first clause or warm-up, on the tokio runtime that asks,
//! and ends when the engine and every clause stream are gone. A clause's stream answers once
//! the server finishes the clause; dropping it (speech out's stop) forgets the context.

mod connection;
#[cfg(test)]
mod live_tests;
mod wire;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use tokio::sync::{mpsc, oneshot};

use self::connection::{Command, Endpoint, Outcome, Speak};
use self::wire::ContextId;
use super::{ElevenLabsOptions, ElevenLabsVoice, OUTPUT_FORMAT, from_provider, voice_path};
use crate::cancel::CancelToken;
use crate::limits::ELEVENLABS_INACTIVITY_TIMEOUT_SECONDS;
use crate::llm::http::endpoint;
use crate::speech_out::{ClauseText, TextToSpeech, TtsChunk, TtsError, TtsStream, VoiceId};

pub struct ElevenLabsRealtime {
    http: Arc<ElevenLabsVoice>,
    endpoint: Arc<Endpoint>,
    connection: Mutex<Option<mpsc::Sender<Command>>>,
    next_context: AtomicU64,
}

impl ElevenLabsRealtime {
    pub fn new(options: ElevenLabsOptions) -> Result<Self, TtsError> {
        let http = Arc::new(ElevenLabsVoice::new(options.clone())?);
        endpoint(options.base_url(), "v1").map_err(from_provider)?;
        // The upgrade needs HTTP/1.1; HTTP/2 would be negotiated otherwise.
        let client = reqwest::Client::builder()
            .http1_only()
            .build()
            .map_err(|error| TtsError::Unavailable {
                detail: crate::llm::bounded_detail(&error.to_string()),
            })?;
        let endpoint = Endpoint {
            client,
            headers: options.key_headers()?,
            base_url: options.base_url().to_owned(),
            model: options.model().to_owned(),
        };
        Ok(Self {
            http,
            endpoint: Arc::new(endpoint),
            connection: Mutex::new(None),
            next_context: AtomicU64::new(0),
        })
    }

    /// The connection task's sender, started on the current runtime when it is not running.
    /// `None` outside a runtime: clauses then go over HTTP.
    fn connection(&self) -> Option<mpsc::Sender<Command>> {
        self.connection_with(None)
    }

    fn connection_with(
        &self,
        recorder: Option<std::sync::mpsc::Sender<String>>,
    ) -> Option<mpsc::Sender<Command>> {
        let mut slot = self.connection.lock().ok()?;
        if let Some(sender) = slot.as_ref().filter(|sender| !sender.is_closed()) {
            return Some(sender.clone());
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let sender = connection::spawn(&runtime, Arc::clone(&self.endpoint), recorder);
        *slot = Some(sender.clone());
        Some(sender)
    }

    fn speak(&self, clause: &ClauseText, voice: &str) -> Option<RealtimeClause> {
        let connection = self.connection()?;
        let context = ContextId(self.next_context.fetch_add(1, Ordering::Relaxed));
        let (reply, outcome) = oneshot::channel();
        let speak = Speak {
            context,
            voice: voice.to_owned(),
            text: clause.text.clone(),
            reply,
        };
        // A full queue never makes a clause wait: it goes over HTTP instead.
        connection.try_send(Command::Speak(speak)).ok()?;
        Some(RealtimeClause {
            outcome,
            _forget: Forget {
                connection,
                context,
            },
        })
    }
}

impl TextToSpeech for ElevenLabsRealtime {
    fn synthesize(&self, clause: &ClauseText, voice: &VoiceId, cancel: CancelToken) -> TtsStream {
        let voice_id = match voice_path(voice) {
            Ok(voice_id) => voice_id,
            Err(error) => return Box::pin(stream::once(std::future::ready(Err(error)))),
        };
        let Some(realtime) = self.speak(clause, voice_id) else {
            return self.http.synthesize(clause, voice, cancel);
        };
        let state = State::Waiting {
            realtime,
            fallback: Fallback {
                http: Arc::clone(&self.http),
                clause: clause.clone(),
                voice: voice.clone(),
            },
            cancel,
        };
        Box::pin(stream::unfold(state, State::next))
    }

    fn prepare(&self, voice: &VoiceId) {
        let Ok(voice_id) = voice_path(voice) else {
            return;
        };
        if let Some(connection) = self.connection() {
            let _ = connection.try_send(Command::WarmUp {
                voice: voice_id.to_owned(),
            });
        }
    }
}

/// The handshake URL: the multi-context endpoint with PCM at 24 kHz, an alignment with every
/// audio message, generation that never waits for more text (`auto_mode`), and the longest idle
/// time the server allows.
fn handshake_url(base_url: &str, voice: &str, model: &str) -> Result<String, TtsError> {
    let path = format!("{base_url}/v1/text-to-speech/{voice}/multi-stream-input");
    let mut url = reqwest::Url::parse(&path).map_err(|_| TtsError::Unavailable {
        detail: "the ElevenLabs base URL is not a valid URL".to_owned(),
    })?;
    url.query_pairs_mut()
        .append_pair("model_id", model)
        .append_pair("output_format", OUTPUT_FORMAT)
        .append_pair("sync_alignment", "true")
        .append_pair("auto_mode", "true")
        .append_pair(
            "inactivity_timeout",
            &ELEVENLABS_INACTIVITY_TIMEOUT_SECONDS.to_string(),
        );
    Ok(url.into())
}

/// A clause sent over the WebSocket, waiting for its answer.
struct RealtimeClause {
    outcome: oneshot::Receiver<Outcome>,
    _forget: Forget,
}

/// Tells the connection to forget a context when its clause's stream goes away.
struct Forget {
    connection: mpsc::Sender<Command>,
    context: ContextId,
}

impl Drop for Forget {
    fn drop(&mut self) {
        // Best effort: a full queue only means the late audio is dropped when it arrives.
        let _ = self.connection.try_send(Command::Cancel(self.context));
    }
}

/// What the HTTP engine needs to speak the clause instead.
struct Fallback {
    http: Arc<ElevenLabsVoice>,
    clause: ClauseText,
    voice: VoiceId,
}

impl Fallback {
    fn start(self, error: &TtsError, cancel: CancelToken) -> TtsStream {
        tracing::info!(%error, "the realtime voice could not speak a clause; using HTTP");
        self.http.synthesize(&self.clause, &self.voice, cancel)
    }
}

enum State {
    Waiting {
        realtime: RealtimeClause,
        fallback: Fallback,
        cancel: CancelToken,
    },
    Emitting(VecDeque<TtsChunk>),
    Http(TtsStream),
    Done,
}

impl State {
    /// The clause's next item; a stop (`cancel`) ends it with nothing more.
    async fn next(self) -> Option<(Result<TtsChunk, TtsError>, Self)> {
        let state = match self {
            Self::Waiting {
                realtime,
                fallback,
                cancel,
            } => {
                let outcome = tokio::select! {
                    outcome = realtime.outcome => outcome,
                    () = cancel.cancelled() => return None,
                };
                match outcome {
                    Ok(Outcome::Complete {
                        audio,
                        words,
                        timing,
                    }) => {
                        tracing::debug!(
                            first_audio_ms = timing.first_audio.map(|at| at.as_millis()),
                            last_audio_ms = timing.last_audio.map(|at| at.as_millis()),
                            finished_ms = timing.finished.as_millis(),
                            messages = timing.messages,
                            fresh_connection = timing.fresh_connection,
                            "realtime clause synthesized"
                        );
                        let mut chunks = VecDeque::from([TtsChunk::Audio(audio)]);
                        chunks.extend(words.map(TtsChunk::Words));
                        Self::Emitting(chunks)
                    }
                    Ok(Outcome::Unavailable { error }) => {
                        Self::Http(fallback.start(&error, cancel))
                    }
                    Ok(Outcome::Failed { error }) => return Some((Err(error), Self::Done)),
                    Err(_closed) => {
                        let error = TtsError::Unavailable {
                            detail: "the realtime connection stopped".to_owned(),
                        };
                        Self::Http(fallback.start(&error, cancel))
                    }
                }
            }
            other @ (Self::Emitting(_) | Self::Http(_) | Self::Done) => other,
        };
        match state {
            Self::Emitting(mut chunks) => {
                let chunk = chunks.pop_front()?;
                Some((Ok(chunk), Self::Emitting(chunks)))
            }
            Self::Http(mut stream) => {
                let item = stream.next().await?;
                Some((item, Self::Http(stream)))
            }
            Self::Waiting { .. } | Self::Done => None,
        }
    }
}
