//! The voice activity state machine (LLD §3.4): Silero probabilities in, voice starts and ends
//! out. Pure, so its edges are tested on synthetic probability sequences.
//!
//! - A window at or above [`SPEECH_THRESHOLD`] may start speech; one below
//!   [`SILENCE_THRESHOLD`] is silence. In between keeps the current state (hysteresis), so a
//!   word that dips does not end the turn.
//! - The first such window after silence is reported at once as `Onset`, so a voice playing
//!   over the user can duck within its 100 ms budget (HLD §8); whether it was speech is only
//!   known at `Start`.
//! - Speech must last [`MIN_SPEECH_MS`] before `Start` is reported; a shorter burst (a click, a
//!   cough) is dropped.
//! - Speech ends after [`HANGOVER_MS`] of silence; `voiced_ms` runs from the onset to the end of
//!   the last speech window, without the hangover.

use crate::audio::MIC_RATE_HZ;

use super::VAD_WINDOW_FRAMES;

/// Probability at or above which a window counts as speech.
pub const SPEECH_THRESHOLD: f32 = 0.5;
/// Probability below which a window counts as silence.
pub const SILENCE_THRESHOLD: f32 = 0.35;
/// Speech shorter than this never starts a turn.
pub const MIN_SPEECH_MS: u32 = 150;
/// Silence this long ends the speech.
pub const HANGOVER_MS: u32 = 250;
/// After this much silence inside speech the final transcript is decoded early, so it is ready
/// when the hangover ends; speech that resumes throws it away. Three windows leave margin for a
/// soft last consonant and still finish a ~100 ms decode before the 256 ms hangover is over.
pub const EARLY_FINAL_AFTER_QUIET_MS: u32 = 96;

const FRAMES_PER_MS: u64 = MIC_RATE_HZ as u64 / 1_000;
const WINDOW: u64 = VAD_WINDOW_FRAMES as u64;

// An onset and its start never fall in the same window, so `step` reports one event at a time.
const _: () = assert!(
    MIN_SPEECH_MS as u64 * FRAMES_PER_MS > WINDOW,
    "the minimum speech must outlast one window"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadEvent {
    /// A window crossed the speech threshold after silence: maybe speech, maybe a cough. A
    /// `Start` with the same `onset_frame` follows only if it lasts [`MIN_SPEECH_MS`].
    Onset { onset_frame: u64 },
    /// Speech started at `onset_frame` (the first frame of its first window).
    Start { onset_frame: u64 },
    /// Speech ended; `voiced_ms` is its length without the hangover.
    End { voiced_ms: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Silence,
    /// Speech may be starting; not reported until it lasts the minimum.
    Candidate {
        onset: u64,
    },
    Speech {
        onset: u64,
        last_speech_end: u64,
        silent_frames: u64,
    },
}

/// Feeds on consecutive windows; frame numbers count from the first window.
#[derive(Clone, Debug)]
pub struct VadMachine {
    state: State,
    next_frame: u64,
}

impl Default for VadMachine {
    fn default() -> Self {
        Self {
            state: State::Silence,
            next_frame: 0,
        }
    }
}

impl VadMachine {
    /// The frame where the next window begins.
    #[must_use]
    pub const fn next_frame(&self) -> u64 {
        self.next_frame
    }

    /// Whether the machine is inside reported speech (between `Start` and `End`).
    #[must_use]
    pub const fn in_speech(&self) -> bool {
        matches!(self.state, State::Speech { .. })
    }

    /// Silence so far inside speech (the hangover running), or 0.
    #[must_use]
    pub fn quiet_ms(&self) -> u32 {
        match self.state {
            State::Speech { silent_frames, .. } => {
                u32::try_from(silent_frames / FRAMES_PER_MS).unwrap_or(u32::MAX)
            }
            State::Silence | State::Candidate { .. } => 0,
        }
    }

    /// Back to silence without reporting anything; the frame count continues.
    pub fn reset(&mut self) {
        self.state = State::Silence;
    }

    /// One window's probability.
    pub fn step(&mut self, probability: f32) -> Option<VadEvent> {
        let begin = self.next_frame;
        let end = begin + WINDOW;
        self.next_frame = end;
        let is_speech = probability >= SPEECH_THRESHOLD;
        let is_silence = probability < SILENCE_THRESHOLD;
        match self.state {
            State::Silence => {
                if is_speech {
                    self.state = State::Candidate { onset: begin };
                    return Some(VadEvent::Onset { onset_frame: begin });
                }
                None
            }
            State::Candidate { onset } => {
                if is_silence {
                    self.state = State::Silence;
                    return None;
                }
                self.confirm(onset, end)
            }
            State::Speech {
                onset,
                last_speech_end,
                silent_frames,
            } => {
                if !is_silence {
                    self.state = State::Speech {
                        onset,
                        last_speech_end: end,
                        silent_frames: 0,
                    };
                    return None;
                }
                let silent_frames = silent_frames + WINDOW;
                if silent_frames >= u64::from(HANGOVER_MS) * FRAMES_PER_MS {
                    self.state = State::Silence;
                    let voiced_ms = (last_speech_end - onset) / FRAMES_PER_MS;
                    return Some(VadEvent::End {
                        voiced_ms: u32::try_from(voiced_ms).unwrap_or(u32::MAX),
                    });
                }
                self.state = State::Speech {
                    onset,
                    last_speech_end,
                    silent_frames,
                };
                None
            }
        }
    }

    /// Reports the start once the candidate has lasted the minimum.
    fn confirm(&mut self, onset: u64, end: u64) -> Option<VadEvent> {
        if end - onset < u64::from(MIN_SPEECH_MS) * FRAMES_PER_MS {
            return None;
        }
        self.state = State::Speech {
            onset,
            last_speech_end: end,
            silent_frames: 0,
        };
        Some(VadEvent::Start { onset_frame: onset })
    }
}

#[cfg(test)]
mod tests {
    use super::{VadEvent, VadMachine};

    /// Runs a probability sequence and returns each event with the index of its window.
    fn run(probabilities: &[f32]) -> Vec<(usize, VadEvent)> {
        let mut machine = VadMachine::default();
        probabilities
            .iter()
            .enumerate()
            .filter_map(|(index, p)| machine.step(*p).map(|event| (index, event)))
            .collect()
    }

    fn repeat(value: f32, count: usize) -> Vec<f32> {
        vec![value; count]
    }

    #[test]
    fn speech_starts_after_the_minimum_and_ends_after_the_hangover() {
        // 10 windows of speech (320 ms), then silence.
        let sequence = [repeat(0.1, 3), repeat(0.9, 10), repeat(0.1, 12)].concat();
        let events = run(&sequence);
        let onset_frame = 3 * 512;
        // The first loud window is reported at once.
        assert_eq!(events[0], (3, VadEvent::Onset { onset_frame }));
        // 150 ms needs 5 windows (160 ms): reported at window 3 + 4.
        assert_eq!(events[1], (7, VadEvent::Start { onset_frame }));
        // 250 ms of silence needs 8 windows (256 ms): window 13 + 7.
        assert_eq!(events[2], (20, VadEvent::End { voiced_ms: 320 }));
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn a_short_burst_is_an_onset_that_never_starts_a_turn() {
        let sequence = [repeat(0.9, 4), repeat(0.1, 20)].concat();
        assert_eq!(
            run(&sequence),
            vec![(0, VadEvent::Onset { onset_frame: 0 })]
        );
    }

    #[test]
    fn each_burst_after_silence_is_a_new_onset_and_speech_has_one() {
        // A cough, silence, a second cough, then speech that dips into the band but never
        // below silence: one onset per rise from silence, and none inside speech.
        let sequence = [
            repeat(0.9, 2),
            repeat(0.1, 3),
            repeat(0.6, 1),
            repeat(0.2, 2),
            repeat(0.9, 3),
            repeat(0.4, 2),
            repeat(0.9, 10),
            repeat(0.1, 9),
        ]
        .concat();
        let onsets: Vec<usize> = run(&sequence)
            .iter()
            .filter(|(_, event)| matches!(event, VadEvent::Onset { .. }))
            .map(|(index, _)| *index)
            .collect();
        assert_eq!(onsets, vec![0, 5, 8]);
        let starts = run(&sequence)
            .iter()
            .filter(|(_, event)| matches!(event, VadEvent::Start { .. }))
            .count();
        assert_eq!(starts, 1);
    }

    #[test]
    fn the_band_between_thresholds_keeps_the_current_state() {
        // Silence then the band: not enough for an onset.
        assert!(run(&repeat(0.4, 20)).is_empty());
        // Speech, then the band: speech goes on and the band counts as voiced.
        let sequence = [repeat(0.9, 6), repeat(0.4, 10), repeat(0.1, 8)].concat();
        let events = run(&sequence);
        assert_eq!(events[2], (23, VadEvent::End { voiced_ms: 512 }));
        // A candidate that dips into the band but not below silence still starts.
        let sequence = [repeat(0.9, 2), repeat(0.4, 3), repeat(0.9, 1)].concat();
        assert_eq!(
            run(&sequence),
            vec![
                (0, VadEvent::Onset { onset_frame: 0 }),
                (4, VadEvent::Start { onset_frame: 0 })
            ]
        );
    }

    #[test]
    fn a_pause_shorter_than_the_hangover_keeps_one_utterance() {
        let sequence = [
            repeat(0.9, 6),
            repeat(0.1, 7),
            repeat(0.9, 6),
            repeat(0.1, 8),
        ]
        .concat();
        let events = run(&sequence);
        assert_eq!(events.len(), 3, "{events:?}");
        // 6 + 7 + 6 windows from onset to the last speech: 19 × 32 ms.
        assert_eq!(events[2].1, VadEvent::End { voiced_ms: 608 });
    }

    #[test]
    fn a_reset_drops_the_utterance_silently() {
        let mut machine = VadMachine::default();
        for _ in 0..6 {
            machine.step(0.9);
        }
        assert!(machine.in_speech());
        machine.reset();
        assert!(!machine.in_speech());
        assert_eq!(machine.next_frame(), 6 * 512);
        for _ in 0..10 {
            assert_eq!(machine.step(0.1), None);
        }
    }
}
