//! One user utterance: transcript segments, voice activity, and when the turn ends (LLD §2.2).

use crate::conversation::cues;
use crate::limits::{
    END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS, UTTERANCE_MAX_BYTES, UTTERANCE_MAX_MS,
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

    /// Continues a turn whose answer was cancelled before anything was heard: the new words
    /// join the words already said (LLD §2.2, Thinking + `VoiceStart`).
    #[must_use]
    pub fn resume(previous: &str, now: SessionTime) -> Self {
        let mut utterance = Self::start(now, true);
        utterance.finals.push_str(previous.trim());
        truncate_at_char(&mut utterance.finals, UTTERANCE_MAX_BYTES);
        utterance
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
    pub fn voice_end(&mut self, now: SessionTime, voiced_ms: u32) {
        self.voice_started_at = None;
        self.voiced_ms = self.voiced_ms.saturating_add(u64::from(voiced_ms));
        self.last_voice_end = Some(now);
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
    /// Silence ends it; a transcript that sounds unfinished earns the grace period instead.
    #[must_use]
    pub fn end_of_turn_at(&self) -> Option<SessionTime> {
        if self.is_voice_active() {
            return None;
        }
        let base = self.last_voice_end.unwrap_or(self.started_at);
        if self.released && self.final_after_voice_end {
            return Some(base);
        }
        let wait_ms = if cues::ends_unfinished(&self.text()) {
            END_OF_TURN_GRACE_MS
        } else {
            END_OF_TURN_SILENCE_MS
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
    use crate::limits::{END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS, UTTERANCE_MAX_MS};
    use crate::time::SessionTime;

    fn at(ms: u64) -> SessionTime {
        SessionTime::from_ms(ms)
    }

    #[test]
    fn silence_ends_a_finished_sentence_and_grace_covers_an_unfinished_one() {
        let mut utterance = Utterance::start(at(0), true);
        assert_eq!(utterance.end_of_turn_at(), None);
        utterance.voice_end(at(1_000), 1_000);
        utterance.transcript("how does the leader get elected", true);
        assert_eq!(
            utterance.end_of_turn_at(),
            Some(at(1_000 + END_OF_TURN_SILENCE_MS))
        );
        utterance.voice_start(at(1_200));
        utterance.voice_end(at(2_000), 800);
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
