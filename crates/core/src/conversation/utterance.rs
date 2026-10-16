//! One user utterance: transcript segments, voice activity, and when the turn ends (LLD §2.2).

use crate::conversation::cues::{self, Completeness};
use crate::limits::{
    END_OF_TURN_FINISHED_SILENCE_MS, END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS,
    UTTERANCE_MAX_BYTES, UTTERANCE_MAX_MS,
};
use crate::time::SessionTime;

/// What the user has said in the current turn. Speech-to-text sends partials that replace each
/// other and one final per voiced segment; the utterance is the finals plus the live partial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utterance {
    started_at: SessionTime,
    finals: String,
    partial: String,
    voice_started_at: Option<SessionTime>,
    voiced_ms: u64,
    /// The last voiced moment of the last segment: `VoiceEnd` minus the silence the detector
    /// already heard. The turn's silence counts from here.
    last_voice_end: Option<SessionTime>,
    final_after_voice_end: bool,
    released: bool,
}

impl Utterance {
    #[must_use]
    pub fn start(now: SessionTime, voice_active: bool) -> Self {
        Self {
            started_at: now,
            finals: String::new(),
            partial: String::new(),
            voice_started_at: voice_active.then_some(now),
            voiced_ms: 0,
            last_voice_end: None,
            final_after_voice_end: false,
            released: false,
        }
    }

    #[must_use]
    pub fn started_at(&self) -> SessionTime {
        self.started_at
    }

    pub fn voice_start(&mut self, now: SessionTime) {
        if self.voice_started_at.is_none() {
            self.voice_started_at = Some(now);
        }
        self.released = false;
    }

    /// `voiced_ms` comes from the voice detector; it is more exact than our own clock span.
    /// `silence_ms` is the silence it heard before calling the end, so the silence that ends the
    /// turn is counted once (it never reaches back before the utterance began).
    pub fn voice_end(&mut self, now: SessionTime, voiced_ms: u32, silence_ms: u32) {
        self.voice_started_at = None;
        self.voiced_ms = self.voiced_ms.saturating_add(u64::from(voiced_ms));
        self.last_voice_end = Some(now.minus_ms(u64::from(silence_ms)).max(self.started_at));
        self.final_after_voice_end = false;
    }

    pub fn transcript(&mut self, text: &str, is_final: bool) {
        if is_final {
            if !self.finals.is_empty() && !text.trim().is_empty() {
                self.finals.push(' ');
            }
            self.finals.push_str(text.trim());
            truncate_at_char(&mut self.finals, UTTERANCE_MAX_BYTES);
            self.partial.clear();
            self.final_after_voice_end = self.voice_started_at.is_none();
        } else {
            self.partial.clear();
            self.partial.push_str(text.trim());
            truncate_at_char(&mut self.partial, UTTERANCE_MAX_BYTES);
        }
    }

    /// Push-to-talk released: the turn ends as soon as the final transcript is in.
    pub fn release(&mut self, now: SessionTime) {
        self.released = true;
        if self.voice_started_at.is_some() {
            self.voice_started_at = None;
            self.last_voice_end = Some(now);
        } else if self.last_voice_end.is_none() {
            self.last_voice_end = Some(now);
        }
    }

    #[must_use]
    pub fn is_voice_active(&self) -> bool {
        self.voice_started_at.is_some()
    }

    /// The final transcript of the last voiced segment is in: its words are known, even when
    /// there are none.
    #[must_use]
    pub fn has_final(&self) -> bool {
        self.final_after_voice_end
    }

    #[must_use]
    pub fn is_released(&self) -> bool {
        self.released
    }

    /// Voice so far, counting a segment still in progress.
    #[must_use]
    pub fn voiced_ms(&self, now: SessionTime) -> u64 {
        let live = self.voice_started_at.map_or(0, |start| now.ms_since(start));
        self.voiced_ms.saturating_add(live)
    }

    #[must_use]
    pub fn text(&self) -> String {
        let mut text = self.finals.clone();
        if !self.partial.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&self.partial);
        }
        truncate_at_char(&mut text, UTTERANCE_MAX_BYTES);
        text
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.finals.trim().is_empty() && self.partial.trim().is_empty()
    }

    /// When the turn ends if nothing else happens, or `None` while the user is speaking.
    /// Silence from the last voiced moment ends it, as long as the words sound (PRODUCT §6.1):
    /// a final transcript that sounds finished ends it soonest, words that sound unfinished
    /// earn the grace period, and anything else waits the usual silence. Only a final can
    /// sound finished: Whisper punctuates partials as if they were the end.
    #[must_use]
    pub fn end_of_turn_at(&self) -> Option<SessionTime> {
        if self.is_voice_active() {
            return None;
        }
        let base = self.last_voice_end.unwrap_or(self.started_at);
        if self.released && self.final_after_voice_end {
            return Some(base);
        }
        let wait_ms = match cues::completeness(&self.text()) {
            Completeness::Unfinished => END_OF_TURN_GRACE_MS,
            Completeness::Finished if self.final_after_voice_end => END_OF_TURN_FINISHED_SILENCE_MS,
            Completeness::Finished | Completeness::Unclear => END_OF_TURN_SILENCE_MS,
        };
        Some(base.plus_ms(wait_ms))
    }

    /// The utterance cap: the turn ends here even while the user keeps talking.
    #[must_use]
    pub fn cap_at(&self) -> SessionTime {
        self.started_at.plus_ms(UTTERANCE_MAX_MS)
    }
}

/// Cuts `text` to at most `max_bytes`, on a char boundary.
pub fn truncate_at_char(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::{Utterance, truncate_at_char};
    use crate::limits::{
        END_OF_TURN_FINISHED_SILENCE_MS, END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS,
        UTTERANCE_MAX_MS,
    };
    use crate::time::SessionTime;

    fn at(ms: u64) -> SessionTime {
        SessionTime::from_ms(ms)
    }

    #[test]
    fn silence_ends_a_finished_sentence_and_grace_covers_an_unfinished_one() {
        let mut utterance = Utterance::start(at(0), true);
        assert_eq!(utterance.end_of_turn_at(), None);
        utterance.voice_end(at(1_000), 1_000, 0);
        utterance.transcript("how does the leader get elected", true);
        assert_eq!(
            utterance.end_of_turn_at(),
            Some(at(1_000 + END_OF_TURN_SILENCE_MS))
        );
        utterance.voice_start(at(1_200));
        utterance.voice_end(at(2_000), 800, 0);
        utterance.transcript("so the leader and", true);
        assert_eq!(
            utterance.text(),
            "how does the leader get elected so the leader and"
        );
        assert_eq!(
            utterance.end_of_turn_at(),
            Some(at(2_000 + END_OF_TURN_GRACE_MS))
        );
        assert_eq!(utterance.voiced_ms(at(5_000)), 1_800);
        assert_eq!(utterance.cap_at(), at(UTTERANCE_MAX_MS));
    }

    /// The detector's hangover is silence already heard: the turn counts it once (the bug
    /// measured in T-010: 250 ms of hangover, then 600 ms more).
    #[test]
    fn the_detectors_silence_counts_toward_the_end_of_the_turn() {
        let mut utterance = Utterance::start(at(0), true);
        utterance.voice_end(at(1_256), 1_000, 256);
        utterance.transcript("show me the code", true);
        assert_eq!(
            utterance.end_of_turn_at(),
            Some(at(1_000 + END_OF_TURN_SILENCE_MS))
        );
        // Never before the utterance began, whatever the detector says.
        let mut short = Utterance::start(at(500), true);
        short.voice_end(at(600), 100, 400);
        assert_eq!(
            short.end_of_turn_at(),
            Some(at(500 + END_OF_TURN_SILENCE_MS))
        );
    }

    /// PRODUCT §6.1: a final that sounds finished ends the turn soonest; a partial never does,
    /// and a trailing-off mark or a continuation word earns the grace.
    #[test]
    fn a_finished_final_ends_the_turn_soonest() {
        let cases: [(&str, bool, u64); 6] = [
            ("Why Redis first?", true, END_OF_TURN_FINISHED_SILENCE_MS),
            ("Okay, go on.", true, END_OF_TURN_FINISHED_SILENCE_MS),
            ("How does a request get?", false, END_OF_TURN_SILENCE_MS),
            ("OK go on", true, END_OF_TURN_SILENCE_MS),
            ("So the leader...", true, END_OF_TURN_GRACE_MS),
            ("So the leader and the.", true, END_OF_TURN_GRACE_MS),
        ];
        for (text, is_final, wait_ms) in cases {
            let mut utterance = Utterance::start(at(0), true);
            utterance.voice_end(at(1_256), 1_000, 256);
            utterance.transcript(text, is_final);
            assert_eq!(
                utterance.end_of_turn_at(),
                Some(at(1_000 + wait_ms)),
                "{text} (final: {is_final})"
            );
        }
    }

    #[test]
    fn partials_replace_each_other_and_finals_accumulate() {
        let mut utterance = Utterance::start(at(0), true);
        utterance.transcript("walk me", false);
        utterance.transcript("walk me through", false);
        assert_eq!(utterance.text(), "walk me through");
        utterance.transcript("Walk me through it.", true);
        utterance.transcript("and", false);
        assert_eq!(utterance.text(), "Walk me through it. and");
    }

    #[test]
    fn a_released_push_to_talk_ends_once_the_final_is_in() {
        let mut utterance = Utterance::start(at(0), true);
        utterance.release(at(900));
        assert_eq!(
            utterance.end_of_turn_at(),
            Some(at(900 + END_OF_TURN_SILENCE_MS))
        );
        utterance.transcript("show me the code", true);
        assert_eq!(utterance.end_of_turn_at(), Some(at(900)));
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let mut text = "aé".to_owned();
        truncate_at_char(&mut text, 2);
        assert_eq!(text, "a");
    }
}
