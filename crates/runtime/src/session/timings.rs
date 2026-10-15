//! Word timings estimated from text, for a voice that has no audio to align (LLD §2.4).
//!
//! The core's simulator has a character-rate estimator too, but it lives behind the `sim`
//! feature, which must never ship; the virtual voice runs in the app until real speech lands.
//! This one also pauses after commas and sentence ends, so captions and strokes keep a
//! speaker's rhythm.

use boardai_core::input::{TimingSource, WordTiming, WordTimings};

use crate::limits::{VIRTUAL_VOICE_COMMA_PAUSE_MS, VIRTUAL_VOICE_SENTENCE_PAUSE_MS};

/// When each word of `text` starts, at `ms_per_byte`, with a pause after `,;:` and `.!?`.
/// Offsets index `text`'s bytes, as the core's speakable offsets do.
#[must_use]
pub fn estimate_timings(text: &str, ms_per_byte: u32) -> WordTimings {
    let mut words = Vec::new();
    let mut now_ms: u32 = 0;
    let mut after_space = true;
    for (offset, ch) in text.char_indices() {
        if after_space && !ch.is_whitespace() {
            words.push(WordTiming {
                byte_offset: u32::try_from(offset).unwrap_or(u32::MAX),
                start_ms: now_ms,
            });
        }
        after_space = ch.is_whitespace();
        let width = u32::try_from(ch.len_utf8()).unwrap_or(1);
        now_ms = now_ms.saturating_add(width.saturating_mul(ms_per_byte));
        let pause = match ch {
            ',' | ';' | ':' => VIRTUAL_VOICE_COMMA_PAUSE_MS,
            '.' | '!' | '?' => VIRTUAL_VOICE_SENTENCE_PAUSE_MS,
            _ => 0,
        };
        now_ms = now_ms.saturating_add(pause);
    }
    WordTimings {
        source: TimingSource::Estimated,
        words,
        duration_ms: now_ms.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::estimate_timings;
    use crate::limits::{VIRTUAL_VOICE_COMMA_PAUSE_MS, VIRTUAL_VOICE_SENTENCE_PAUSE_MS};

    #[test]
    fn words_start_in_order_with_pauses_after_punctuation() {
        let text = "Sure, it starts here. Then";
        let timings = estimate_timings(text, 10);
        let offsets: Vec<u32> = timings.words.iter().map(|word| word.byte_offset).collect();
        assert_eq!(offsets, vec![0, 6, 9, 16, 22]);
        let starts: Vec<u32> = timings.words.iter().map(|word| word.start_ms).collect();
        assert_eq!(starts[1], 60 + VIRTUAL_VOICE_COMMA_PAUSE_MS);
        assert_eq!(
            starts[4],
            220 + VIRTUAL_VOICE_COMMA_PAUSE_MS + VIRTUAL_VOICE_SENTENCE_PAUSE_MS
        );
        assert!(starts.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            timings.duration_ms,
            260 + VIRTUAL_VOICE_COMMA_PAUSE_MS + VIRTUAL_VOICE_SENTENCE_PAUSE_MS
        );
    }

    #[test]
    fn empty_text_still_has_a_length() {
        let timings = estimate_timings("", 65);
        assert!(timings.words.is_empty());
        assert_eq!(timings.duration_ms, 1);
    }
}
