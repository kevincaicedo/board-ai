//! The streaming session every provider shares: connect, retry before the first token, read the
//! body (SSE, or newline-delimited JSON for Ollama's native API) under deadlines, and hand events
//! to a provider-specific [`Decoder`].
//!
//! The stream owns the HTTP response, so dropping the stream drops the connection: that is the
//! cancellation path, and no task is spawned.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use boardai_core::time::SessionTime;
use futures_util::stream;
use tokio::time::Instant;

use super::error::{ProviderError, TimeoutKind, bounded_detail};
use super::http::{self, RequestSpec};
use super::ndjson::NdjsonParser;
use super::sse::{SseEvent, SseParser};
use super::{ChatEvent, ChatStream, RetryPolicy};
use crate::cancel::CancelToken;
use crate::clock::Clock;
use crate::limits::{PROVIDER_RESPONSE_MAX_BYTES, PROVIDER_RETRIES_MAX};

/// Turns one provider's SSE events into chat events.
pub(crate) trait Decoder: Send + 'static {
    /// Handles one event, appending what it completes to `out`.
    fn on_event(
        &mut self,
        event: SseEvent,
        out: &mut VecDeque<ChatEvent>,
    ) -> Result<Flow, ProviderError>;

    /// The body ended before `Flow::Done`: either complete it (push `Done`) or fail.
    fn on_end_of_body(&mut self, out: &mut VecDeque<ChatEvent>) -> Result<(), ProviderError>;

    /// Forgets a failed attempt before a retry.
    fn reset(&mut self);
}

pub(crate) enum Flow {
    Continue,
    Done,
}

/// How a provider frames its streamed body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    /// Server-Sent Events (Anthropic, OpenAI-compatible servers).
    Sse,
    /// One JSON object per line (Ollama's native API); each line reaches the decoder as an
    /// event's data.
    NdJson,
}

/// The framing's parser.
#[derive(Debug)]
enum Framer {
    Sse(SseParser),
    NdJson(NdjsonParser),
}

impl Framer {
    fn new(framing: Framing) -> Self {
        match framing {
            Framing::Sse => Self::Sse(SseParser::new()),
            Framing::NdJson => Self::NdJson(NdjsonParser::new()),
        }
    }

    fn push(&mut self, bytes: &[u8], out: &mut Vec<SseEvent>) -> Result<(), String> {
        match self {
            Self::Sse(parser) => parser.push(bytes, out).map_err(|error| error.to_string()),
            Self::NdJson(parser) => parser.push(bytes, out).map_err(|error| error.to_string()),
        }
    }

    fn finish(&mut self) {
        let _discarded = match self {
            Self::Sse(parser) => parser.finish(),
            Self::NdJson(parser) => parser.finish(),
        };
    }

    fn framing(&self) -> Framing {
        match self {
            Self::Sse(_) => Framing::Sse,
            Self::NdJson(_) => Framing::NdJson,
        }
    }
}

pub(crate) struct SessionParts {
    pub client: reqwest::Client,
    pub request: Result<RequestSpec, ProviderError>,
    pub policy: RetryPolicy,
    pub clock: Arc<dyn Clock>,
    pub provider: &'static str,
    pub framing: Framing,
}

/// Starts a stream. Nothing happens until it is polled.
pub(crate) fn open<D: Decoder>(parts: SessionParts, decoder: D, cancel: CancelToken) -> ChatStream {
    let request = match parts.request {
        Ok(request) => request,
        Err(error) => return Box::pin(stream::once(std::future::ready(Err(error)))),
    };
    let session = Session {
        client: parts.client,
        request,
        policy: parts.policy,
        clock: parts.clock,
        provider: parts.provider,
        decoder,
        parser: Framer::new(parts.framing),
        scratch: Vec::new(),
        pending: VecDeque::new(),
        deferred_error: None,
        cancel,
        body: None,
        deadline: None,
        started_at: None,
        attempt: 0,
        produced: false,
        finished: false,
        received_bytes: 0,
    };
    Box::pin(stream::unfold(session, |mut session| async move {
        let item = session.next_item().await?;
        Some((item, session))
    }))
}

struct Session<D> {
    client: reqwest::Client,
    request: RequestSpec,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
    provider: &'static str,
    decoder: D,
    parser: Framer,
    scratch: Vec<SseEvent>,
    pending: VecDeque<ChatEvent>,
    /// An error that arrived after events still to be delivered; it goes out after them.
    deferred_error: Option<ProviderError>,
    cancel: CancelToken,
    body: Option<reqwest::Response>,
    deadline: Option<Instant>,
    started_at: Option<SessionTime>,
    attempt: u32,
    /// A chat event has been produced: from here on nothing is retried.
    produced: bool,
    finished: bool,
    received_bytes: usize,
}

enum Step {
    Continue,
    Cancelled,
    Failed(ProviderError),
}

impl<D: Decoder> Session<D> {
    async fn next_item(&mut self) -> Option<Result<ChatEvent, ProviderError>> {
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + self.policy.first_token_deadline);
        if self.started_at.is_none() {
            self.started_at = Some(self.clock.now());
        }
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if let Some(error) = self.deferred_error.take() {
                self.finish();
                return Some(Err(error));
            }
            if self.finished {
                return None;
            }
            let step = if self.body.is_some() {
                self.read(deadline).await
            } else {
                self.connect(deadline).await
            };
            match step {
                Step::Continue => {}
                Step::Cancelled => {
                    self.finish();
                    return None;
                }
                Step::Failed(error) => self.on_failure(error, deadline).await?,
            }
        }
    }

    /// Retries when nothing was produced yet and the budget allows; otherwise queues the error.
    /// `None` means the stream was cancelled while waiting.
    async fn on_failure(&mut self, error: ProviderError, deadline: Instant) -> Option<()> {
        let Some(delay) = self.retry_delay(&error, deadline) else {
            self.deferred_error = Some(error);
            return Some(());
        };
        tracing::info!(provider = self.provider, attempt = self.attempt + 1, %error, "retrying");
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                self.finish();
                None
            }
            () = tokio::time::sleep(delay) => {
                self.attempt += 1;
                self.body = None;
                self.parser = Framer::new(self.parser.framing());
                self.decoder.reset();
                self.received_bytes = 0;
                Some(())
            }
        }
    }

    fn retry_delay(&self, error: &ProviderError, deadline: Instant) -> Option<Duration> {
        if self.produced
            || !self.pending.is_empty()
            || !error.is_retryable()
            || self.attempt >= PROVIDER_RETRIES_MAX
        {
            return None;
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let seed =
            self.clock.now().as_ms() ^ SEQUENCE.fetch_add(1, Ordering::Relaxed).rotate_left(32);
        let delay = http::retry_delay(
            self.attempt,
            self.policy.backoff_base,
            error.retry_after(),
            seed,
        )?;
        (Instant::now() + delay < deadline).then_some(delay)
    }

    async fn connect(&mut self, deadline: Instant) -> Step {
        let request = self
            .client
            .post(&self.request.url)
            .headers(self.request.headers.clone())
            .body(self.request.body.clone());
        let sent = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Step::Cancelled,
            sent = tokio::time::timeout_at(deadline, request.send()) => sent,
        };
        match sent {
            Err(_elapsed) => Step::Failed(ProviderError::Timeout {
                kind: TimeoutKind::FirstToken,
            }),
            Ok(Err(error)) => Step::Failed(network_error(error)),
            Ok(Ok(response)) if response.status().is_success() => {
                self.body = Some(response);
                Step::Continue
            }
            Ok(Ok(response)) => Step::Failed(http::error_from_response(response).await),
        }
    }

    async fn read(&mut self, deadline: Instant) -> Step {
        let (wait_until, kind) = if self.produced {
            (Instant::now() + self.policy.idle_timeout, TimeoutKind::Idle)
        } else {
            (deadline, TimeoutKind::FirstToken)
        };
        let Some(body) = self.body.as_mut() else {
            return Step::Continue;
        };
        let chunk = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Step::Cancelled,
            chunk = tokio::time::timeout_at(wait_until, body.chunk()) => chunk,
        };
        match chunk {
            Err(_elapsed) => Step::Failed(ProviderError::Timeout { kind }),
            Ok(Err(error)) => Step::Failed(network_error(error)),
            Ok(Ok(None)) => self.end_of_body(),
            Ok(Ok(Some(bytes))) => self.feed(&bytes),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Step {
        self.received_bytes = self.received_bytes.saturating_add(bytes.len());
        if self.received_bytes > PROVIDER_RESPONSE_MAX_BYTES {
            return Step::Failed(ProviderError::BadOutput {
                detail: format!("the response exceeds {PROVIDER_RESPONSE_MAX_BYTES} bytes"),
            });
        }
        let mut events = std::mem::take(&mut self.scratch);
        // Events completed before a bad line are still decoded, so where the chunk boundaries
        // fall never changes what the consumer sees.
        let parsed = self.parser.push(bytes, &mut events);
        let mut step = Step::Continue;
        for event in events.drain(..) {
            if !matches!(step, Step::Continue) || self.finished {
                break;
            }
            match self.decoder.on_event(event, &mut self.pending) {
                Ok(Flow::Continue) => {}
                Ok(Flow::Done) => {
                    self.finished = true;
                    self.body = None;
                }
                Err(error) => step = Step::Failed(error),
            }
        }
        if let (Step::Continue, false, Err(detail)) = (&step, self.finished, parsed) {
            step = Step::Failed(ProviderError::BadOutput { detail });
        }
        events.clear();
        self.scratch = events;
        self.note_first_token();
        step
    }

    fn end_of_body(&mut self) -> Step {
        self.body = None;
        // A trailing unterminated event cannot complete a response, so it is dropped; whether
        // the response is complete is the decoder's call.
        self.parser.finish();
        let step = match self.decoder.on_end_of_body(&mut self.pending) {
            Ok(()) => {
                self.finished = true;
                Step::Continue
            }
            Err(error) => Step::Failed(error),
        };
        self.note_first_token();
        step
    }

    fn note_first_token(&mut self) {
        if self.produced || self.pending.is_empty() {
            return;
        }
        self.produced = true;
        let started_at = self.started_at.unwrap_or_default();
        let latency_ms = self.clock.now().ms_since(started_at);
        tracing::debug!(
            provider = self.provider,
            latency_ms,
            attempt = self.attempt,
            "first token"
        );
    }

    fn finish(&mut self) {
        self.finished = true;
        self.body = None;
        self.pending.clear();
    }
}

fn network_error(error: reqwest::Error) -> ProviderError {
    // The URL is left out: a base URL may carry credentials.
    ProviderError::Network {
        detail: bounded_detail(&error.without_url().to_string()),
    }
}
