//! A speech out with no audio: it estimates word timings from the text and "plays" each clause
//! in real time, reporting exactly what the real speech out will report (LLD §3.1): a
//! `ClauseReady` per clause, positions on the playback watch every 20 ms, `PlaybackDrained`
//! once everything has played, and `VoiceStopped` after a 30 ms fade. It drives the text dev
//! loop and the app until the speech stream lands.

use std::collections::VecDeque;
use std::time::Duration;

use boardai_core::effect::Gain;
use boardai_core::ids::ClauseId;
use boardai_core::input::PlaybackPosition;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::inbox::{SessionClosed, SpeechOutLink};
use super::ports::{SpeechOut, SpeechOutError};
use super::timings::estimate_timings;
use crate::limits::{
    PLAYBACK_REPORT_MS, VIRTUAL_VOICE_COMMANDS_MAX, VIRTUAL_VOICE_MS_PER_BYTE,
    VIRTUAL_VOICE_SPEED_MAX, VIRTUAL_VOICE_SPEED_MIN, VOICE_STOP_FADE_MS,
};

/// How fast the virtual voice speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualVoiceConfig {
    pub ms_per_byte: u32,
}

impl VirtualVoiceConfig {
    /// `speed` 2.0 speaks twice as fast; it is clamped to
    /// [`VIRTUAL_VOICE_SPEED_MIN`]–[`VIRTUAL_VOICE_SPEED_MAX`]. Faster speech, not faster
    /// time: the session clock stays real, so budgets and captions stay consistent.
    #[must_use]
    pub fn with_speed(speed: f32) -> Self {
        let speed = if speed.is_finite() {
            speed.clamp(VIRTUAL_VOICE_SPEED_MIN, VIRTUAL_VOICE_SPEED_MAX)
        } else {
            1.0
        };
        let base = f64::from(VIRTUAL_VOICE_MS_PER_BYTE);
        // In [1, base / SPEED_MIN] by the clamp, so the saturating cast never saturates.
        let ms_per_byte = (base / f64::from(speed)).round().max(1.0) as u32;
        Self { ms_per_byte }
    }
}

impl Default for VirtualVoiceConfig {
    fn default() -> Self {
        Self {
            ms_per_byte: VIRTUAL_VOICE_MS_PER_BYTE,
        }
    }
}

#[derive(Debug)]
enum Command {
    Synthesize { clause: ClauseId, text: String },
    Duck(Gain),
    Stop,
    Reset,
}

/// The port the orchestrator holds. Dropping it ends the actor.
#[derive(Debug)]
pub struct VirtualVoice {
    commands: mpsc::Sender<Command>,
    task: JoinHandle<()>,
}

impl VirtualVoice {
    /// Starts the voice's actor; it answers through `link`.
    #[must_use]
    pub fn spawn(link: SpeechOutLink, config: VirtualVoiceConfig) -> Self {
        let (commands, receiver) = mpsc::channel(VIRTUAL_VOICE_COMMANDS_MAX);
        let actor = Actor {
            link,
            config,
            queue: VecDeque::new(),
            playing: None,
            drain_pending: false,
            gain: Gain::FULL,
        };
        let task = tokio::spawn(actor.run(receiver));
        Self { commands, task }
    }

    fn command(&self, command: Command) -> Result<(), SpeechOutError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => SpeechOutError::Full,
                mpsc::error::TrySendError::Closed(_) => SpeechOutError::Closed,
            })
    }
}

impl Drop for VirtualVoice {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SpeechOut for VirtualVoice {
    fn synthesize(&mut self, clause: ClauseId, text: String) -> Result<(), SpeechOutError> {
        self.command(Command::Synthesize { clause, text })
    }

    fn duck(&mut self, gain: Gain) {
        // A lost duck changes only a volume the virtual voice doesn't have.
        let _ = self.command(Command::Duck(gain));
    }

    fn stop(&mut self) -> Result<(), SpeechOutError> {
        self.command(Command::Stop)
    }

    fn reset(&mut self) {
        let _ = self.command(Command::Reset);
    }
}

#[derive(Clone, Copy, Debug)]
struct Clip {
    clause: ClauseId,
    duration_ms: u32,
}

#[derive(Clone, Copy, Debug)]
struct Playing {
    clip: Clip,
    started: Instant,
}

impl Playing {
    fn offset_ms(&self, now: Instant) -> u32 {
        let elapsed =
            u32::try_from(now.duration_since(self.started).as_millis()).unwrap_or(u32::MAX);
        elapsed.min(self.clip.duration_ms)
    }
}

struct Actor {
    link: SpeechOutLink,
    config: VirtualVoiceConfig,
    queue: VecDeque<Clip>,
    playing: Option<Playing>,
    /// Something played since the last `PlaybackDrained`.
    drain_pending: bool,
    gain: Gain,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let mut ticker = tokio::time::interval(Duration::from_millis(PLAYBACK_REPORT_MS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let active = self.playing.is_some() || !self.queue.is_empty() || self.drain_pending;
            let outcome = tokio::select! {
                biased;
                command = commands.recv() => match command {
                    Some(command) => self.handle(command).await,
                    None => return,
                },
                _ = ticker.tick(), if active => self.advance(Instant::now()).await,
            };
            if outcome.is_err() {
                return;
            }
        }
    }

    async fn handle(&mut self, command: Command) -> Result<(), SessionClosed> {
        match command {
            Command::Synthesize { clause, text } => {
                let timings = estimate_timings(&text, self.config.ms_per_byte);
                self.queue.push_back(Clip {
                    clause,
                    duration_ms: timings.duration_ms,
                });
                self.link.clause_ready(clause, timings).await
            }
            Command::Duck(gain) => {
                self.gain = gain;
                tracing::trace!(gain = self.gain.value(), "virtual voice gain");
                Ok(())
            }
            Command::Stop => {
                let now = Instant::now();
                let fade = Duration::from_millis(VOICE_STOP_FADE_MS);
                // The fade still plays, so it counts as heard.
                let last = self.playing.take().map(|playing| PlaybackPosition {
                    clause: playing.clip.clause,
                    offset_ms: playing.offset_ms(now + fade),
                });
                self.queue.clear();
                self.drain_pending = false;
                tokio::time::sleep(fade).await;
                self.link.voice_stopped(last).await
            }
            Command::Reset => {
                self.queue.clear();
                self.playing = None;
                self.drain_pending = false;
                Ok(())
            }
        }
    }

    /// One playback report: the clause playing now and where, then the drain once nothing is
    /// left.
    async fn advance(&mut self, now: Instant) -> Result<(), SessionClosed> {
        // A clause that ended hands over to the next in the same report, so at most two pass.
        for _ in 0..2 {
            if self.playing.is_none() {
                let Some(clip) = self.queue.pop_front() else {
                    break;
                };
                self.playing = Some(Playing { clip, started: now });
                self.drain_pending = true;
            }
            let Some(playing) = self.playing else {
                break;
            };
            let offset_ms = playing.offset_ms(now);
            self.link.playback_at(PlaybackPosition {
                clause: playing.clip.clause,
                offset_ms,
            });
            if offset_ms < playing.clip.duration_ms {
                return Ok(());
            }
            self.playing = None;
        }
        if self.playing.is_none() && self.queue.is_empty() && self.drain_pending {
            self.drain_pending = false;
            return self.link.drained().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
