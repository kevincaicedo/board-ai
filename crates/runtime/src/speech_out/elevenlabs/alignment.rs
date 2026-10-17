//! ElevenLabs' character alignment turned into the core's word timings (LLD §2.4): each word
//! starts where its first character starts. The alignment must spell the clause exactly (up to
//! trailing whitespace, which the realtime protocol asks us to add) and fit inside its audio;
//! anything else gives no timings, and speech out estimates them rather than guessing.
//!
//! Where a message's times count from is found from the times themselves. The realtime API
//! reference says each message ("relative to the returned chunk"), but the live check found
//! them counting from the clause's start across messages (a 1,745 ms clause had its last words
//! at 1,983 and 2,100 ms when read per message). So times that go back mark a new origin at the
//! message's audio; times that go on continue the current origin. Either reading of the docs
//! then gives the same timings.

use boardai_core::input::WordTiming;

use crate::limits::ELEVENLABS_ALIGNMENT_CHARACTERS_MAX;
use crate::speech_out::timings::word_starts;

/// Times at least this far below the previous start (ms) are a restart. Less is jitter in how
/// a message's last whitespace is timed, never a new origin (which goes back to about zero).
const RESTART_MARGIN_MS: f64 = 50.0;

/// A word may start this far past the audio (ms) and still be inside it: rounding.
const AUDIO_END_TOLERANCE_MS: f64 = 1.0;

/// Aligned characters in order, each with its start in ms from the start of the clause's audio.
#[derive(Debug, Default)]
pub(crate) struct Alignment {
    characters: Vec<(String, f64)>,
    /// Characters past [`ELEVENLABS_ALIGNMENT_CHARACTERS_MAX`]; any makes the timings estimated.
    dropped_count: usize,
    /// The previous start as the provider wrote it.
    last_written_ms: Option<f64>,
    /// Where the provider's times count from in the clause's audio.
    origin_ms: f64,
}

impl Alignment {
    /// One message's characters with their starts as written; its audio begins at
    /// `message_start_ms` in the clause.
    pub(crate) fn push_message(
        &mut self,
        characters: impl IntoIterator<Item = (String, f64)>,
        message_start_ms: f64,
    ) {
        let mut characters = characters.into_iter().peekable();
        if let (Some((_, first)), Some(last)) = (characters.peek(), self.last_written_ms)
            && *first + RESTART_MARGIN_MS <= last
        {
            self.origin_ms = message_start_ms;
        }
        for (character, written_ms) in characters {
            self.last_written_ms = Some(written_ms);
            if self.characters.len() >= ELEVENLABS_ALIGNMENT_CHARACTERS_MAX {
                self.dropped_count += 1;
                continue;
            }
            let start_ms = (written_ms + self.origin_ms).max(0.0);
            self.characters.push((character, start_ms));
        }
    }

    /// Word starts as byte offsets into `text`, when the characters spell it and start inside
    /// the clause's `audio_ms`.
    pub(crate) fn word_timings(&self, text: &str, audio_ms: f64) -> Option<Vec<WordTiming>> {
        if self.characters.is_empty() || self.dropped_count > 0 {
            return None;
        }
        let mut spelled = String::new();
        // Byte offset in `spelled` where each aligned character begins, with its start.
        let mut starts: Vec<(usize, f64)> = Vec::with_capacity(self.characters.len());
        for (character, start) in &self.characters {
            starts.push((spelled.len(), *start));
            spelled.push_str(character);
        }
        if spelled.trim_end() != text.trim_end() {
            return None;
        }
        let words: Vec<WordTiming> = word_starts(text)
            .into_iter()
            .filter_map(|offset| {
                let index = starts.partition_point(|(begin, _)| *begin < offset as usize);
                let (begin, start) = starts.get(index)?;
                (*begin == offset as usize).then(|| WordTiming {
                    byte_offset: offset,
                    start_ms: start.round() as u32,
                })
            })
            .collect();
        // A word starting past the audio means the times were read from the wrong origin.
        let inside =
            |word: &WordTiming| f64::from(word.start_ms) <= audio_ms + AUDIO_END_TOLERANCE_MS;
        words.iter().all(inside).then_some(words)
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::input::WordTiming;

    use super::Alignment;
    use crate::limits::ELEVENLABS_ALIGNMENT_CHARACTERS_MAX;

    fn message(characters: &[(&str, f64)]) -> Vec<(String, f64)> {
        characters
            .iter()
            .map(|(character, start)| ((*character).to_owned(), *start))
            .collect()
    }

    fn pairs(words: &[WordTiming]) -> Vec<(u32, u32)> {
        words
            .iter()
            .map(|word| (word.byte_offset, word.start_ms))
            .collect()
    }

    #[test]
    fn words_start_at_their_first_character_in_bytes() {
        let mut alignment = Alignment::default();
        let characters = [
            ("C", 0.0),
            ("a", 40.0),
            ("f", 80.0),
            ("é", 120.0),
            (" ", 160.0),
            ("o", 200.4),
            ("k", 240.6),
            (" ", 260.0),
        ];
        alignment.push_message(message(&characters), 0.0);
        // The trailing space the realtime protocol adds is not part of the clause.
        let words = alignment.word_timings("Café ok", 400.0).expect("spelled");
        assert_eq!(pairs(&words), vec![(0, 0), (6, 200)]);
    }

    #[test]
    fn times_that_continue_keep_their_origin_and_times_that_restart_take_the_message_start() {
        // What the wire does: the second message's times go on from the first's.
        let mut continuing = Alignment::default();
        continuing.push_message(message(&[("a", 0.0), (" ", 300.0)]), 0.0);
        continuing.push_message(message(&[("b", 983.0), (" ", 1_100.0)]), 1_000.0);
        let words = continuing.word_timings("a b", 1_745.0).expect("spelled");
        assert_eq!(pairs(&words), vec![(0, 0), (2, 983)]);
        // A last space timed a little late is jitter, not a restart.
        let mut jittery = Alignment::default();
        jittery.push_message(message(&[("a", 0.0), (" ", 150.0)]), 0.0);
        jittery.push_message(message(&[("b", 120.0)]), 100.0);
        let words = jittery.word_timings("a b", 400.0).expect("spelled");
        assert_eq!(pairs(&words), vec![(0, 0), (2, 120)]);
        // What the reference says: each message's times start again from its own audio.
        let mut restarting = Alignment::default();
        restarting.push_message(message(&[("a", 0.0), (" ", 300.0)]), 0.0);
        restarting.push_message(message(&[("b", 0.0), (" ", 100.0)]), 1_000.0);
        restarting.push_message(message(&[("c", 150.0)]), 1_400.0);
        let words = restarting.word_timings("a b c", 1_745.0).expect("spelled");
        assert_eq!(pairs(&words), vec![(0, 0), (2, 1_000), (4, 1_150)]);
    }

    #[test]
    fn a_different_spelling_too_many_characters_or_starts_past_the_audio_give_no_timings() {
        let mut different = Alignment::default();
        different.push_message(message(&[("H", 0.0), ("e", 1.0), ("y", 2.0)]), 0.0);
        assert!(different.word_timings("Hi", 100.0).is_none());
        assert!(Alignment::default().word_timings("Hi", 100.0).is_none());
        let mut long = Alignment::default();
        let characters =
            (0..=ELEVENLABS_ALIGNMENT_CHARACTERS_MAX).map(|index| ("a".to_owned(), index as f64));
        long.push_message(characters, 0.0);
        let text = "a".repeat(ELEVENLABS_ALIGNMENT_CHARACTERS_MAX + 1);
        assert!(long.word_timings(&text, 10_000.0).is_none());
        let mut past = Alignment::default();
        past.push_message(message(&[("a", 0.0), (" ", 300.0), ("b", 1_983.0)]), 0.0);
        assert!(past.word_timings("a b", 1_745.0).is_none());
        assert!(past.word_timings("a b", 2_000.0).is_some());
    }
}
