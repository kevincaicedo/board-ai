//! The speech-out actor: one tokio task between the orchestrator and the voice ring.
//!
//! It synthesizes clauses in order, at most [`SPEECH_LOOKAHEAD_CLAUSES_MAX`] clauses or
//! [`SPEECH_LOOKAHEAD_MS_MAX`] ahead of what has been heard (past that, synthesis waits for
//! playback). Each finished clause is announced with `ClauseReady` before its first frame enters
//! the ring, so the core always knows a clause's timings before it hears positions in it.
//!
//! What has been heard comes from the played-frames counter minus the output latency. While the
//! counter stands still (the ring ran dry), the frames already sent keep reaching the speaker for
//! one latency; the heard position follows the clock over that tail so `PlaybackDrained` comes
//! when the last frame is actually heard.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::effect::{Effect, Gain};
use boardai_core::ids::ClauseId;
use boardai_core::input::{Input, PlaybackPosition, WordTiming};
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::timeline::Timeline;
use super::timings::{self, duration_ms};
use super::{ClauseText, TextToSpeech, TtsChunk, TtsError, TtsStream, VoiceId};
use crate::audio::{VOICE_RATE_HZ, VoiceOutput};
use crate::cancel::CancelToken;
use crate::limits::{
    PLAYBACK_REPORT_INTERVAL_MS, SPEECH_LOOKAHEAD_CLAUSES_MAX, SPEECH_LOOKAHEAD_MS_MAX,
    SPEECH_OUT_COMMAND_QUEUE, SPEECH_OUT_EVENT_QUEUE, TTS_CLAUSE_AUDIO_MAX_MS,
    TTS_CLAUSE_TIMEOUT_MS, VOICE_STOP_ACK_TIMEOUT_MS,
};

const LOOKAHEAD_FRAMES: u64 = SPEECH_LOOKAHEAD_MS_MAX as u64 * VOICE_RATE_HZ as u64 / 1_000;
const CLAUSE_FRAMES_MAX: usize =
    (TTS_CLAUSE_AUDIO_MAX_MS as u64 * VOICE_RATE_HZ as u64 / 1_000) as usize;

/// What the orchestrator asks of speech out: the speech effects, plus a reset.
#[derive(Clone, Debug, PartialEq)]
pub enum SpeechOutCommand {
    Synthesize {
        clause: ClauseId,
        text: String,
    },
    Duck {
        gain: Gain,
    },
    /// Fade out and answer with `Input::VoiceStopped`.
    StopVoice,
    /// Stop everything without an answer and restore full gain (a new session, mute).
    Reset,
}

impl SpeechOutCommand {
    /// The command for a speech effect; `None` for effects speech out does not handle.
    #[must_use]
    pub fn from_effect(effect: &Effect) -> Option<Self> {
        match effect {
            Effect::Duck { gain } => Some(Self::Duck { gain: *gain }),
            Effect::StopVoice => Some(Self::StopVoice),
            Effect::Synthesize { clause, text } => Some(Self::Synthesize {
                clause: *clause,
                text: text.clone(),
            }),
            Effect::Talk { .. }
            | Effect::CancelTalk
            | Effect::RunTool { .. }
            | Effect::RunHarness { .. }
            | Effect::CancelHarness { .. }
            | Effect::Ui(_)
            | Effect::Log(_)
            | Effect::Remember(_)
            | Effect::WakeAt { .. }
            | Effect::Forget { .. }
            | Effect::AcceptProposal { .. } => None,
        }
    }
}

pub struct SpeechOutConfig {
    pub voice: VoiceId,
    /// Tried for a clause the main engine fails on (LLD §3.4: the system voice).
    pub fallback: Option<(Arc<dyn TextToSpeech>, VoiceId)>,
}

/// What speech out reports. `events` carries `ClauseReady`, `PlaybackDrained` and
/// `VoiceStopped` in order; `playback` holds the latest position (`None` when nothing plays),
/// which the orchestrator steps into the core as `Input::PlaybackAt`.
pub struct SpeechOutChannels {
    pub events: mpsc::Receiver<Input>,
    pub playback: watch::Receiver<Option<PlaybackPosition>>,
}

/// The handle that owns the actor. Dropping every command sender stops it; `shutdown` also
/// waits for it to finish.
pub struct SpeechOut {
    commands: mpsc::Sender<SpeechOutCommand>,
    task: JoinHandle<()>,
}

impl SpeechOut {
    /// Starts the actor on the current tokio runtime.
    pub fn spawn(
        engine: Arc<dyn TextToSpeech>,
        config: SpeechOutConfig,
        output: VoiceOutput,
    ) -> (Self, SpeechOutChannels) {
        let (commands, command_receiver) = mpsc::channel(SPEECH_OUT_COMMAND_QUEUE);
        let (events, event_receiver) = mpsc::channel(SPEECH_OUT_EVENT_QUEUE);
        let (playback, playback_receiver) = watch::channel(None);
        let actor = Actor {
            engine,
            config,
            output,
            events,
            playback,
            queue: VecDeque::new(),
            synthesis: None,
            ready: VecDeque::new(),
            timeline: Timeline::default(),
            stopping: None,
            heard: HeardClock::default(),
            drained: true,
        };
        let task = tokio::spawn(actor.run(command_receiver));
        let channels = SpeechOutChannels {
            events: event_receiver,
            playback: playback_receiver,
        };
        (Self { commands, task }, channels)
    }

    /// A sender for the orchestrator. Sends wait while [`SPEECH_OUT_COMMAND_QUEUE`] commands
    /// are queued.
    #[must_use]
    pub fn sender(&self) -> mpsc::Sender<SpeechOutCommand> {
        self.commands.clone()
    }

    /// Stops the voice, ends the actor and waits for it. Other senders must be dropped too.
    pub async fn shutdown(self) {
        drop(self.commands);
        let _ = self.task.await;
    }
}

/// One clause being synthesized.
struct Synthesis {
    clause: ClauseText,
    stream: TtsStream,
    cancel: CancelToken,
    deadline: Instant,
    on_fallback: bool,
    audio: Vec<f32>,
    words: Vec<WordTiming>,
    cut_frames: usize,
}

impl Drop for Synthesis {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// A synthesized clause whose audio is not all in the ring yet. It is placed on the timeline
/// when its first frame enters the ring, so a stop never strands it on a timeline it reset.
struct ReadyClause {
    clause: ClauseId,
    audio: Vec<f32>,
    written: usize,
    placed: bool,
}

struct Stopping {
    answers: usize,
    deadline: Instant,
    /// The answer already given when the device did not acknowledge in time.
    answered_early: bool,
}

/// Turns the played counter into the heard counter (see the module comment).
#[derive(Default)]
struct HeardClock {
    played: u64,
    since: Option<Instant>,
    /// What was last reported heard. The latency grows when the audio unit moves to a slower
    /// device (Bluetooth), and what was heard is never taken back.
    heard: u64,
}

impl HeardClock {
    fn heard(&mut self, played: u64, latency: u64, now: Instant) -> u64 {
        if played != self.played || self.since.is_none() {
            self.played = played;
            self.since = Some(now);
        }
        let elapsed = self.since.map_or(Duration::ZERO, |since| now - since);
        let elapsed_frames = elapsed.as_millis() as u64 * u64::from(VOICE_RATE_HZ) / 1_000;
        let heard = played.saturating_sub(latency) + elapsed_frames.min(latency.min(played));
        self.heard = self.heard.max(heard);
        self.heard
    }
}

enum Next {
    Command(Option<SpeechOutCommand>),
    Chunk(Option<Result<TtsChunk, TtsError>>),
    Tick,
}

struct Actor {
    engine: Arc<dyn TextToSpeech>,
    config: SpeechOutConfig,
    output: VoiceOutput,
    events: mpsc::Sender<Input>,
    playback: watch::Sender<Option<PlaybackPosition>>,
    queue: VecDeque<ClauseText>,
    synthesis: Option<Synthesis>,
    ready: VecDeque<ReadyClause>,
    timeline: Timeline,
    stopping: Option<Stopping>,
    heard: HeardClock,
    /// `PlaybackDrained` was sent for everything in the timeline.
    drained: bool,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<SpeechOutCommand>) {
        let mut ticker = tokio::time::interval(Duration::from_millis(PLAYBACK_REPORT_INTERVAL_MS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            self.start_synthesis();
            let next = tokio::select! {
                command = commands.recv() => Next::Command(command),
                chunk = next_chunk(&mut self.synthesis) => Next::Chunk(chunk),
                _ = ticker.tick() => Next::Tick,
            };
            let open = match next {
                Next::Command(Some(command)) => {
                    self.on_command(command);
                    true
                }
                Next::Command(None) => false,
                Next::Chunk(chunk) => self.on_chunk(chunk).await,
                Next::Tick => self.on_tick().await,
            };
            if !open {
                break;
            }
        }
        self.stop(false);
    }

    fn on_command(&mut self, command: SpeechOutCommand) {
        match command {
            SpeechOutCommand::Synthesize { clause, text } => {
                self.queue.push_back(ClauseText { id: clause, text });
            }
            SpeechOutCommand::Duck { gain } => self.output.set_gain(gain),
            SpeechOutCommand::StopVoice => self.stop(true),
            SpeechOutCommand::Reset => {
                self.stop(false);
                self.output.set_gain(Gain::FULL);
            }
        }
    }

    /// Fades the voice out and forgets every clause not yet heard. `answer` asks for a
    /// `VoiceStopped` once the device acknowledges.
    fn stop(&mut self, answer: bool) {
        self.output.request_stop();
        self.queue.clear();
        self.synthesis = None;
        self.ready.clear();
        self.playback.send_replace(None);
        let deadline = Instant::now() + Duration::from_millis(VOICE_STOP_ACK_TIMEOUT_MS);
        let stopping = self.stopping.get_or_insert(Stopping {
            answers: 0,
            deadline,
            answered_early: false,
        });
        if answer {
            stopping.answers += 1;
            stopping.deadline = deadline;
            stopping.answered_early = false;
        }
    }

    /// Starts the next clause when the lookahead allows it.
    fn start_synthesis(&mut self) {
        if self.synthesis.is_some() || self.queue.is_empty() {
            return;
        }
        let heard = self.heard_frames(Instant::now());
        let unplaced = self.ready.iter().filter(|clause| !clause.placed);
        let unplaced_frames: u64 = unplaced
            .clone()
            .map(|clause| clause.audio.len() as u64)
            .sum();
        let ahead_clauses = self.timeline.unheard_clauses(heard) + unplaced.count();
        let ahead_frames = self.timeline.end_frame().saturating_sub(heard) + unplaced_frames;
        if ahead_clauses >= SPEECH_LOOKAHEAD_CLAUSES_MAX || ahead_frames >= LOOKAHEAD_FRAMES {
            return;
        }
        if let Some(clause) = self.queue.pop_front() {
            let engine = Arc::clone(&self.engine);
            let voice = self.config.voice.clone();
            self.synthesis = Some(begin(&*engine, &voice, clause, false));
        }
    }

    /// Handles one synthesis result. Returns whether the actor keeps running.
    async fn on_chunk(&mut self, chunk: Option<Result<TtsChunk, TtsError>>) -> bool {
        let Some(synthesis) = self.synthesis.as_mut() else {
            return true;
        };
        match chunk {
            Some(Ok(TtsChunk::Audio(samples))) => {
                let room = CLAUSE_FRAMES_MAX.saturating_sub(synthesis.audio.len());
                let kept = samples.len().min(room);
                synthesis.audio.extend_from_slice(&samples[..kept]);
                synthesis.cut_frames += samples.len() - kept;
                true
            }
            Some(Ok(TtsChunk::Words(words))) => {
                synthesis.words.extend(words);
                true
            }
            Some(Err(error)) => self.on_failure(error).await,
            None => self.finish_clause().await,
        }
    }

    /// A clause failed: try the fallback voice once, else report it so the core skips it and
    /// captions the problem (D-059). Returns whether the actor keeps running.
    async fn on_failure(&mut self, error: TtsError) -> bool {
        let Some(failed) = self.synthesis.take() else {
            return true;
        };
        let clause = failed.clause.clone();
        let on_fallback = failed.on_fallback;
        drop(failed);
        match (&self.config.fallback, on_fallback) {
            (Some((fallback, voice)), false) => {
                tracing::warn!(clause = clause.id.0, %error, "speech engine failed; using the fallback voice");
                self.synthesis = Some(begin(&**fallback, voice, clause, true));
            }
            (Some(_), true) | (None, _) => {
                tracing::error!(clause = clause.id.0, %error, "speech failed; clause skipped");
                let failed = Input::SynthesisFailed { clause: clause.id };
                return self.events.send(failed).await.is_ok();
            }
        }
        true
    }

    /// The clause is complete: announce its timings, then queue its audio for the ring.
    async fn finish_clause(&mut self) -> bool {
        let Some(mut done) = self.synthesis.take() else {
            return true;
        };
        if done.cut_frames > 0 {
            tracing::warn!(
                clause = done.clause.id.0,
                cut_frames = done.cut_frames,
                "clause audio cut at its limit"
            );
        }
        let audio = std::mem::take(&mut done.audio);
        let timings = if done.words.is_empty() {
            timings::estimate(&done.clause.text, &audio)
        } else {
            timings::exact(&done.clause.text, &done.words, duration_ms(audio.len()))
        };
        let ready = Input::ClauseReady {
            clause: done.clause.id,
            timings,
        };
        if self.events.send(ready).await.is_err() {
            return false;
        }
        self.drained = false;
        self.ready.push_back(ReadyClause {
            clause: done.clause.id,
            audio,
            written: 0,
            placed: false,
        });
        self.fill_ring();
        true
    }

    /// Moves synthesized audio into the ring, in clause order, as far as it has room.
    fn fill_ring(&mut self) {
        if self.stopping.is_some() {
            return;
        }
        while let Some(front) = self.ready.front_mut() {
            if !front.placed {
                self.timeline.append(front.clause, front.audio.len() as u64);
                front.placed = true;
            }
            front.written += self.output.push(&front.audio[front.written..]);
            if front.written < front.audio.len() {
                return;
            }
            self.ready.pop_front();
        }
    }

    async fn on_tick(&mut self) -> bool {
        let now = Instant::now();
        if self.stopping.is_some() {
            return self.check_stop(now).await;
        }
        self.fill_ring();
        let heard = self.heard_frames(now);
        let position = self.timeline.position(heard);
        self.playback.send_if_modified(|current| {
            let changed = *current != position && position.is_some();
            if changed {
                *current = position;
            }
            changed
        });
        self.timeline.retire(heard);
        let idle = self.queue.is_empty() && self.synthesis.is_none() && self.ready.is_empty();
        if idle && !self.drained && heard >= self.timeline.end_frame() {
            self.drained = true;
            self.playback.send_replace(None);
            return self.events.send(Input::PlaybackDrained).await.is_ok();
        }
        true
    }

    /// Answers stops once the device acknowledges (or from the counter, after the timeout).
    async fn check_stop(&mut self, now: Instant) -> bool {
        let Some(stopping) = self.stopping.as_mut() else {
            return true;
        };
        let acknowledged = self.output.stop_acknowledged();
        let early = acknowledged.is_none() && now >= stopping.deadline && !stopping.answered_early;
        if acknowledged.is_none() && !early {
            return true;
        }
        let played =
            acknowledged.map_or_else(|| self.output.played_frames(), |ack| ack.played_frames);
        let last = self.timeline.position(played);
        let answers = std::mem::take(&mut stopping.answers);
        if early {
            tracing::warn!("the audio device did not acknowledge the stop in time");
            stopping.answered_early = true;
        } else {
            self.stopping = None;
            self.timeline.reset(played);
            self.heard = HeardClock::default();
            self.drained = self.ready.is_empty();
        }
        for _ in 0..answers {
            if self
                .events
                .send(Input::VoiceStopped { last })
                .await
                .is_err()
            {
                return false;
            }
        }
        self.fill_ring();
        true
    }

    fn heard_frames(&mut self, now: Instant) -> u64 {
        let played = self.output.played_frames();
        let latency = self.output.output_latency_frames();
        self.heard.heard(played, latency, now)
    }
}

fn begin(
    engine: &dyn TextToSpeech,
    voice: &VoiceId,
    clause: ClauseText,
    on_fallback: bool,
) -> Synthesis {
    let cancel = CancelToken::new();
    let stream = engine.synthesize(&clause, voice, cancel.clone());
    Synthesis {
        clause,
        stream,
        cancel,
        deadline: Instant::now() + Duration::from_millis(TTS_CLAUSE_TIMEOUT_MS),
        on_fallback,
        audio: Vec::new(),
        words: Vec::new(),
        cut_frames: 0,
    }
}

/// The next item of the current synthesis; never resolves when there is none. A clause past its
/// deadline fails with `Timeout`.
async fn next_chunk(synthesis: &mut Option<Synthesis>) -> Option<Result<TtsChunk, TtsError>> {
    let Some(synthesis) = synthesis.as_mut() else {
        return std::future::pending().await;
    };
    match tokio::time::timeout_at(synthesis.deadline, synthesis.stream.next()).await {
        Ok(item) => item,
        Err(_elapsed) => Some(Err(TtsError::Timeout)),
    }
}
