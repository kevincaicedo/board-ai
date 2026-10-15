//! A fake speech-out: word timings estimated from a character rate, playback reported every
//! [`PLAYBACK_REPORT_MS`], and `VoiceStopped` answering `StopVoice` after a short fade, like
//! the runtime's speech actor (LLD §3.1).

use std::collections::{BTreeMap, VecDeque};

use crate::ids::ClauseId;
use crate::input::{Input, PlaybackPosition, TimingSource, WordTiming, WordTimings};
use crate::time::SessionTime;

/// How often playback is reported, as the runtime coalesces it.
pub const PLAYBACK_REPORT_MS: u64 = 20;

/// The fade-out after `StopVoice`; audio keeps playing through it.
pub const STOP_FADE_MS: u64 = 30;

/// Word timings for `text` at `ms_per_byte`: a word starts at each non-space after a space.
#[must_use]
pub fn estimate_timings(text: &str, ms_per_byte: u32) -> WordTimings {
    let mut words = Vec::new();
    let mut after_space = true;
    for (offset, ch) in text.char_indices() {
        if after_space && !ch.is_whitespace() {
            let byte_offset = u32::try_from(offset).unwrap_or(u32::MAX);
            words.push(WordTiming {
                byte_offset,
                start_ms: byte_offset.saturating_mul(ms_per_byte),
            });
        }
        after_space = ch.is_whitespace();
    }
    let length = u32::try_from(text.len()).unwrap_or(u32::MAX);
    WordTimings {
        source: TimingSource::Estimated,
        words,
        duration_ms: length.saturating_mul(ms_per_byte).max(1),
    }
}

#[derive(Clone, Debug)]
struct Clip {
    clause: ClauseId,
    timings: WordTimings,
    ready: bool,
}

#[derive(Clone, Copy, Debug)]
struct Playing {
    clause: ClauseId,
    started_at: SessionTime,
    duration_ms: u64,
}

#[derive(Clone, Debug)]
pub struct FakeSpeech {
    pub ms_per_byte: u32,
    pub synthesis_ms: u64,
    clips: VecDeque<Clip>,
    playing: Option<Playing>,
    gain: f32,
    drained_reported: bool,
    /// When each clause started playing, with its timings: what was actually heard.
    started: BTreeMap<ClauseId, (SessionTime, WordTimings)>,
    order: Vec<ClauseId>,
}

impl Default for FakeSpeech {
    fn default() -> Self {
        Self {
            ms_per_byte: 60,
            synthesis_ms: 40,
            clips: VecDeque::new(),
            playing: None,
            gain: 1.0,
            drained_reported: true,
            started: BTreeMap::new(),
            order: Vec::new(),
        }
    }
}

impl FakeSpeech {
    /// Queues a clause; returns its `ClauseReady`, to deliver after the synthesis delay.
    pub fn synthesize(&mut self, clause: ClauseId, text: &str) -> Input {
        let timings = estimate_timings(text, self.ms_per_byte);
        self.clips.push_back(Clip {
            clause,
            timings: timings.clone(),
            ready: false,
        });
        Input::ClauseReady { clause, timings }
    }

    pub fn mark_ready(&mut self, clause: ClauseId) {
        if let Some(clip) = self.clips.iter_mut().find(|clip| clip.clause == clause) {
            clip.ready = true;
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    #[must_use]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Something is playing or waiting to play.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.playing.is_some() || !self.clips.is_empty()
    }

    /// One playback report tick.
    pub fn tick(&mut self, now: SessionTime) -> Vec<Input> {
        let mut inputs = Vec::new();
        // At most one clause ends and one starts per tick, plus the drain report.
        for _ in 0..2 {
            if self.playing.is_none() {
                self.start_next(now);
            }
            let Some(playing) = self.playing else {
                break;
            };
            let offset_ms = now.ms_since(playing.started_at).min(playing.duration_ms);
            inputs.push(Input::PlaybackAt {
                position: PlaybackPosition {
                    clause: playing.clause,
                    offset_ms: u32::try_from(offset_ms).unwrap_or(u32::MAX),
                },
            });
            if offset_ms < playing.duration_ms {
                break;
            }
            self.playing = None;
        }
        let waiting = self.clips.front().is_some_and(|clip| clip.ready);
        if self.playing.is_none() && !waiting && !self.drained_reported {
            self.drained_reported = true;
            inputs.push(Input::PlaybackDrained);
        }
        inputs
    }

    fn start_next(&mut self, now: SessionTime) {
        if !self.clips.front().is_some_and(|clip| clip.ready) {
            return;
        }
        let Some(clip) = self.clips.pop_front() else {
            return;
        };
        self.playing = Some(Playing {
            clause: clip.clause,
            started_at: now,
            duration_ms: u64::from(clip.timings.duration_ms),
        });
        self.started.insert(clip.clause, (now, clip.timings));
        self.order.push(clip.clause);
        self.drained_reported = false;
    }

    /// `StopVoice`: the audio fades for [`STOP_FADE_MS`] and stops; everything queued is
    /// dropped. Returns the last position played.
    pub fn stop(&mut self, now: SessionTime) -> Option<PlaybackPosition> {
        self.clips.clear();
        self.drained_reported = true;
        let playing = self.playing.take()?;
        let offset_ms = now
            .plus_ms(STOP_FADE_MS)
            .ms_since(playing.started_at)
            .min(playing.duration_ms);
        let offset_ms = u32::try_from(offset_ms).unwrap_or(u32::MAX);
        if let Some((_, timings)) = self.started.get_mut(&playing.clause) {
            // Words after the fade were never heard.
            timings.words.retain(|word| word.start_ms <= offset_ms);
            timings.duration_ms = offset_ms;
        }
        Some(PlaybackPosition {
            clause: playing.clause,
            offset_ms,
        })
    }

    /// When the audio at speakable `offset` of `clause` played, if that clause ever started.
    #[must_use]
    pub fn audio_time(&self, clause: ClauseId, offset: usize) -> Option<SessionTime> {
        let (started_at, timings) = self.started.get(&clause)?;
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        let start_ms = timings
            .words
            .iter()
            .find(|word| word.byte_offset >= offset)
            .map_or(timings.duration_ms, |word| word.start_ms);
        Some(started_at.plus_ms(u64::from(start_ms)))
    }

    /// Start times of every word that began playing, in order.
    #[must_use]
    pub fn word_starts(&self) -> Vec<SessionTime> {
        self.order
            .iter()
            .filter_map(|clause| self.started.get(clause))
            .flat_map(|(started_at, timings)| {
                timings
                    .words
                    .iter()
                    .map(move |word| started_at.plus_ms(u64::from(word.start_ms)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeSpeech, estimate_timings};
    use crate::ids::ClauseId;
    use crate::input::Input;
    use crate::time::SessionTime;

    #[test]
    fn plays_ready_clauses_in_order_and_reports_the_drain() {
        let timings = estimate_timings("one two", 10);
        assert_eq!(timings.words.len(), 2);
        assert_eq!(timings.duration_ms, 70);
        let mut speech = FakeSpeech {
            ms_per_byte: 10,
            ..FakeSpeech::default()
        };
        let _ = speech.synthesize(ClauseId(0), "one two");
        speech.mark_ready(ClauseId(0));
        let first = speech.tick(SessionTime::from_ms(100));
        assert!(matches!(first.as_slice(), [Input::PlaybackAt { .. }]));
        let last = speech.tick(SessionTime::from_ms(200));
        assert!(matches!(last.last(), Some(Input::PlaybackDrained)));
        assert_eq!(speech.word_starts().len(), 2);
        assert_eq!(
            speech.audio_time(ClauseId(0), 4),
            Some(SessionTime::from_ms(140))
        );
    }
}
