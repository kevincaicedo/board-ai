//! Backchannel or interruption (LLD §2.2). The voice is already ducked when this runs; the
//! decision only says whether it comes back up or stops.

use crate::conversation::cues;
use crate::conversation::utterance::Utterance;
use crate::limits::{
    BARGE_IN_DECISION_MS, BARGE_IN_POINTER_WINDOW_MS, BARGE_IN_VOICE_MS, BARGE_IN_WORDS_MIN,
};
use crate::time::SessionTime;

/// The user started speaking over BoardAI at `onset`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BargeIn {
    onset: SessionTime,
    utterance: Utterance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Undecided,
    Backchannel,
    Interruption(InterruptionReason),
}

/// Which rule decided. Kept for tests and the log; every reason stops the voice the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptionReason {
    /// (a) A cue among the first words.
    Cue,
    /// (b) Four or more words.
    Words,
    /// (c) At least 700 ms of voice.
    Voice,
    /// (d) A pointer target set within 1.5 s of the onset, and final words that are not a
    /// backchannel.
    Pointer,
    /// (e) The voice ended and its final words are not a backchannel: a short remark ("Redis?")
    /// is said to BoardAI. It used to wait for the timeout, with the same outcome.
    Remark,
    /// Neither rule applied within the decision window: a person who keeps talking wants the
    /// floor.
    Timeout,
}

impl BargeIn {
    #[must_use]
    pub fn new(onset: SessionTime) -> Self {
        Self {
            onset,
            utterance: Utterance::start(onset, true),
        }
    }

    #[must_use]
    pub fn onset(&self) -> SessionTime {
        self.onset
    }

    #[must_use]
    pub fn decide_by(&self) -> SessionTime {
        self.onset.plus_ms(BARGE_IN_DECISION_MS)
    }

    #[must_use]
    pub fn utterance(&self) -> &Utterance {
        &self.utterance
    }

    pub fn utterance_mut(&mut self) -> &mut Utterance {
        &mut self.utterance
    }

    #[must_use]
    pub fn into_utterance(self) -> Utterance {
        self.utterance
    }

    /// Applies the rules in the LLD's order. `pointer_set_at` is when the pointer last landed on
    /// a target, if it is still on one.
    ///
    /// Rules (a) to (c) decide at once, from partials too. Past them, the final transcript of
    /// the voiced segment decides: a backchannel keeps the voice, anything else interrupts
    /// (rules (d) and (e)). A partial is never trusted for that: "got" of "got it" looks like
    /// more than a backchannel, and "yes" of "yes go ahead" like one. The runtime decodes the
    /// final while the detector's hangover runs, so it comes with `VoiceEnd` and the wait costs
    /// nothing. A pointer only names the reason: the UI reports a hover as a pointer, and a mouse
    /// drifting over a node while the user says "mm-hm" is no request for the floor (I7, review
    /// finding 23).
    #[must_use]
    pub fn decide(&self, now: SessionTime, pointer_set_at: Option<SessionTime>) -> Decision {
        let text = self.utterance.text();
        let voiced_ms = self.utterance.voiced_ms(now);
        if cues::has_interruption_cue(&text) {
            return Decision::Interruption(InterruptionReason::Cue);
        }
        if cues::word_count(&text) >= BARGE_IN_WORDS_MIN {
            return Decision::Interruption(InterruptionReason::Words);
        }
        if voiced_ms >= u64::from(BARGE_IN_VOICE_MS) {
            return Decision::Interruption(InterruptionReason::Voice);
        }
        // Measured from the onset, so a decision that waits for the words still sees it.
        let pointing = pointer_set_at
            .is_some_and(|set_at| set_at.plus_ms(BARGE_IN_POINTER_WINDOW_MS) >= self.onset);
        let words_known = !self.utterance.is_voice_active() && self.utterance.has_final();
        if words_known {
            return match (cues::is_backchannel(&text), pointing) {
                (true, _) => Decision::Backchannel,
                (false, true) => Decision::Interruption(InterruptionReason::Pointer),
                (false, false) => Decision::Interruption(InterruptionReason::Remark),
            };
        }
        if now >= self.decide_by() {
            return Decision::Interruption(InterruptionReason::Timeout);
        }
        Decision::Undecided
    }
}

#[cfg(test)]
mod tests {
    use super::{BargeIn, Decision, InterruptionReason};
    use crate::conversation::cues::BACKCHANNELS;
    use crate::limits::{BARGE_IN_DECISION_MS, BARGE_IN_POINTER_WINDOW_MS};
    use crate::time::SessionTime;

    fn at(ms: u64) -> SessionTime {
        SessionTime::from_ms(ms)
    }

    /// I7 at the rule level: every backchannel, said briefly, is a backchannel once its final is
    /// in; its partial alone decides nothing.
    #[test]
    fn every_short_backchannel_keeps_the_voice() {
        for phrase in BACKCHANNELS {
            let mut barge_in = BargeIn::new(at(3_200));
            barge_in.utterance_mut().transcript(phrase, false);
            barge_in.utterance_mut().voice_end(at(3_550), 350, 0);
            assert_eq!(
                barge_in.decide(at(3_550), None),
                Decision::Undecided,
                "{phrase}"
            );
            barge_in.utterance_mut().transcript(phrase, true);
            assert_eq!(
                barge_in.decide(at(3_551), None),
                Decision::Backchannel,
                "{phrase}"
            );
        }
    }

    /// A partial that reads as a backchannel ("yes" of "yes go ahead") is not one.
    #[test]
    fn a_backchannel_partial_waits_for_its_final() {
        let mut barge_in = BargeIn::new(at(3_200));
        barge_in.utterance_mut().transcript("yes", false);
        barge_in.utterance_mut().voice_end(at(4_040), 840, 256);
        assert_eq!(barge_in.decide(at(4_040), None), Decision::Undecided);
        barge_in.utterance_mut().transcript("yes go ahead", true);
        assert_eq!(
            barge_in.decide(at(4_041), None),
            Decision::Interruption(InterruptionReason::Remark)
        );
    }

    #[test]
    fn each_rule_decides_an_interruption() {
        let cases: [(&str, u32, Option<u64>, InterruptionReason); 5] = [
            ("wait", 300, None, InterruptionReason::Cue),
            (
                "redis first then postgres",
                600,
                None,
                InterruptionReason::Words,
            ),
            ("hmm", 1_250, None, InterruptionReason::Voice),
            ("this one", 400, Some(1_800), InterruptionReason::Pointer),
            ("Redis?", 400, None, InterruptionReason::Remark),
        ];
        for (text, voiced_ms, pointer, reason) in cases {
            let mut barge_in = BargeIn::new(at(3_200));
            barge_in
                .utterance_mut()
                .voice_end(at(3_200 + u64::from(voiced_ms)), voiced_ms, 0);
            barge_in.utterance_mut().transcript(text, true);
            let decision = barge_in.decide(at(3_700), pointer.map(at));
            assert_eq!(decision, Decision::Interruption(reason), "{text}");
        }
    }

    /// I7 at the measured lengths (`spike_turns`): the longest backchannel `say` produced, 960 ms
    /// of detector voice, stays a backchannel even judged live just before its `VoiceEnd`, while
    /// the hangover past `VoiceStart`'s lag still counts as voice. At the old 700 ms it did not.
    #[test]
    fn the_longest_measured_backchannel_keeps_the_voice() {
        const LONGEST_BACKCHANNEL_MS: u32 = 960;
        // The 256 ms hangover less the 160 ms `VoiceStart` comes after the onset.
        const HANGOVER_PAST_START_MS: u64 = 96;
        for phrase in BACKCHANNELS {
            let start = at(3_200);
            let mut barge_in = BargeIn::new(start);
            barge_in.utterance_mut().transcript(phrase, false);
            let end = start.plus_ms(u64::from(LONGEST_BACKCHANNEL_MS) + HANGOVER_PAST_START_MS);
            assert_eq!(
                barge_in.decide(end.minus_ms(1), None),
                Decision::Undecided,
                "{phrase}"
            );
            barge_in
                .utterance_mut()
                .voice_end(end, LONGEST_BACKCHANNEL_MS, 256);
            barge_in.utterance_mut().transcript(phrase, true);
            assert_eq!(
                barge_in.decide(end.plus_ms(50), None),
                Decision::Backchannel,
                "{phrase}"
            );
        }
    }

    #[test]
    fn an_old_pointer_does_not_count_and_silence_past_the_window_interrupts() {
        let mut barge_in = BargeIn::new(at(10_000));
        barge_in.utterance_mut().transcript("this", false);
        let stale = at(10_000 - BARGE_IN_POINTER_WINDOW_MS - 1);
        assert_eq!(
            barge_in.decide(at(10_400), Some(stale)),
            Decision::Undecided
        );
        let late = at(10_000 + BARGE_IN_DECISION_MS);
        assert_eq!(
            barge_in.decide(late, None),
            Decision::Interruption(InterruptionReason::Voice)
        );
        barge_in.utterance_mut().voice_end(at(10_300), 300, 0);
        assert_eq!(
            barge_in.decide(late, None),
            Decision::Interruption(InterruptionReason::Timeout)
        );
    }

    /// I7 with the mouse on the board: a hover just before (or during) a backchannel leaves it a
    /// backchannel (review finding 23).
    #[test]
    fn a_backchannel_while_pointing_stays_a_backchannel() {
        for phrase in BACKCHANNELS {
            let mut barge_in = BargeIn::new(at(3_200));
            let first = phrase.split(' ').next().unwrap_or(phrase);
            barge_in.utterance_mut().transcript(first, false);
            assert_eq!(
                barge_in.decide(at(3_500), Some(at(3_000))),
                Decision::Undecided,
                "a partial waits: {phrase}"
            );
            barge_in.utterance_mut().voice_end(at(3_550), 350, 0);
            barge_in.utterance_mut().transcript(phrase, true);
            assert_eq!(
                barge_in.decide(at(3_670), Some(at(3_000))),
                Decision::Backchannel,
                "{phrase}"
            );
        }
    }

    /// Pointing with no words yet: the final transcript decides. "this" interrupts; an empty
    /// final is a backchannel.
    #[test]
    fn pointing_with_no_words_yet_waits_for_the_final() {
        let pointer = Some(at(3_000));
        let mut this = BargeIn::new(at(3_200));
        this.utterance_mut().voice_end(at(3_480), 280, 0);
        assert_eq!(this.decide(at(3_480), pointer), Decision::Undecided);
        this.utterance_mut().transcript("this", true);
        assert_eq!(
            this.decide(at(3_600), pointer),
            Decision::Interruption(InterruptionReason::Pointer)
        );
        let mut nothing = BargeIn::new(at(3_200));
        nothing.utterance_mut().voice_end(at(3_480), 280, 0);
        nothing.utterance_mut().transcript("", true);
        assert_eq!(nothing.decide(at(3_600), pointer), Decision::Backchannel);
    }
}
