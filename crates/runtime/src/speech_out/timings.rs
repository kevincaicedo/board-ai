//! Word timings in the core's terms (LLD §2.4): byte offsets into the clause's speakable text,
//! milliseconds from the start of the clause's audio.
//!
//! Engines index text in other units (the system voice in UTF-16 code units, ElevenLabs in
//! characters), so this module converts them. An engine that gives no timings gets estimated
//! ones: the character rate scaled to the voiced part of the real audio.

use boardai_core::input::{TimingSource, WordTiming, WordTimings};

use crate::audio::VOICE_RATE_HZ;

/// Blocks of this length decide where the voice starts and ends in an estimate.
const VOICED_BLOCK_MS: u32 = 10;

/// A block is voiced when its RMS reaches this fraction of the loudest block's RMS.
const VOICED_RELATIVE_LEVEL: f32 = 0.1;

/// Milliseconds of `frames` at 24 kHz, saturating.
#[must_use]
pub fn duration_ms(frames: usize) -> u32 {
    u32::try_from(frames as u64 * 1_000 / u64::from(VOICE_RATE_HZ)).unwrap_or(u32::MAX)
}

/// Byte offsets where words start: a non-whitespace character at the start or after
/// whitespace.
#[must_use]
pub fn word_starts(text: &str) -> Vec<u32> {
    let mut starts = Vec::new();
    let mut previous_is_space = true;
    for (offset, character) in text.char_indices() {
        let is_space = character.is_whitespace();
        if previous_is_space && !is_space {
            starts.push(u32::try_from(offset).unwrap_or(u32::MAX));
        }
        previous_is_space = is_space;
    }
    starts
}

/// The byte offset of the character at UTF-16 index `index`; the text's length when `index` is
/// at the end; `None` past the end or inside a surrogate pair.
#[must_use]
pub fn utf16_to_byte(text: &str, index: usize) -> Option<u32> {
    let mut utf16 = 0;
    for (offset, character) in text.char_indices() {
        if utf16 == index {
            return u32::try_from(offset).ok();
        }
        if utf16 > index {
            return None;
        }
        utf16 += character.len_utf16();
    }
    (utf16 == index)
        .then(|| u32::try_from(text.len()).ok())
        .flatten()
}

/// The byte offset of the character at `index` (counting Unicode scalar values).
#[must_use]
pub fn char_to_byte(text: &str, index: usize) -> Option<u32> {
    if index == text.chars().count() {
        return u32::try_from(text.len()).ok();
    }
    text.char_indices()
        .nth(index)
        .and_then(|(offset, _)| u32::try_from(offset).ok())
}

/// Exact timings from an engine, checked at the boundary: offsets must fall on character
/// boundaries inside the text, starts are clamped to the audio, sorted by offset, and kept
/// non-decreasing in time so the core never sees a word start before the one it follows.
#[must_use]
pub fn exact(text: &str, words: &[WordTiming], duration_ms: u32) -> WordTimings {
    let mut kept: Vec<WordTiming> = words
        .iter()
        .filter(|word| {
            let offset = word.byte_offset as usize;
            offset < text.len() && text.is_char_boundary(offset)
        })
        .map(|word| WordTiming {
            byte_offset: word.byte_offset,
            start_ms: word.start_ms.min(duration_ms),
        })
        .collect();
    kept.sort_by_key(|word| word.byte_offset);
    kept.dedup_by_key(|word| word.byte_offset);
    let mut latest = 0;
    for word in &mut kept {
        word.start_ms = word.start_ms.max(latest);
        latest = word.start_ms;
    }
    WordTimings {
        source: TimingSource::Exact,
        words: kept,
        duration_ms,
    }
}

/// Estimated timings: each word starts where its characters fall when the text is spread
/// evenly over the voiced part of `pcm` (24 kHz).
#[must_use]
pub fn estimate(text: &str, pcm: &[f32]) -> WordTimings {
    let duration = duration_ms(pcm.len());
    let (voiced_begin_ms, voiced_end_ms) = voiced_span_ms(pcm).unwrap_or((0, duration));
    let voiced_ms = u64::from(voiced_end_ms.saturating_sub(voiced_begin_ms));
    let total_chars = text.chars().count().max(1) as u64;
    let words = word_starts(text)
        .into_iter()
        .map(|byte_offset| {
            let chars_before = text[..byte_offset as usize].chars().count() as u64;
            let start = u64::from(voiced_begin_ms) + voiced_ms * chars_before / total_chars;
            WordTiming {
                byte_offset,
                start_ms: u32::try_from(start).unwrap_or(u32::MAX).min(duration),
            }
        })
        .collect();
    WordTimings {
        source: TimingSource::Estimated,
        words,
        duration_ms: duration,
    }
}

/// The span from the first to the last voiced block, or `None` for silence.
fn voiced_span_ms(pcm: &[f32]) -> Option<(u32, u32)> {
    let block = (VOICED_BLOCK_MS * VOICE_RATE_HZ / 1_000) as usize;
    let levels: Vec<f32> = pcm
        .chunks(block)
        .map(|chunk| {
            let energy: f32 = chunk.iter().map(|sample| sample * sample).sum();
            (energy / chunk.len() as f32).sqrt()
        })
        .collect();
    let loudest = levels.iter().copied().fold(0.0_f32, f32::max);
    if loudest <= f32::EPSILON {
        return None;
    }
    let threshold = loudest * VOICED_RELATIVE_LEVEL;
    let first = levels.iter().position(|level| *level >= threshold)?;
    let last = levels.iter().rposition(|level| *level >= threshold)?;
    let to_ms = |blocks: usize| u32::try_from(blocks).unwrap_or(u32::MAX) * VOICED_BLOCK_MS;
    let end_ms = to_ms(last + 1).min(duration_ms(pcm.len()));
    Some((to_ms(first), end_ms))
}

#[cfg(test)]
mod tests {
    use boardai_core::input::{TimingSource, WordTiming};

    use super::{char_to_byte, estimate, exact, utf16_to_byte, word_starts};

    #[test]
    fn word_starts_are_byte_offsets() {
        assert_eq!(word_starts("  the café, then naïve"), vec![2, 6, 13, 18]);
        assert!(word_starts(" \n").is_empty());
    }

    #[test]
    fn utf16_indices_map_to_bytes() {
        let text = "a😀 café";
        // 'a' 1 unit, emoji 2 units, space 1, 'c' at 4.
        assert_eq!(utf16_to_byte(text, 0), Some(0));
        assert_eq!(utf16_to_byte(text, 1), Some(1));
        assert_eq!(utf16_to_byte(text, 2), None);
        assert_eq!(utf16_to_byte(text, 4), Some(6));
        assert_eq!(utf16_to_byte(text, 8), Some(11));
        assert_eq!(utf16_to_byte(text, 9), None);
    }

    #[test]
    fn character_indices_map_to_bytes() {
        let text = "naïve app";
        assert_eq!(char_to_byte(text, 3), Some(4));
        assert_eq!(char_to_byte(text, 6), Some(7));
        assert_eq!(char_to_byte(text, 9), Some(10));
        assert_eq!(char_to_byte(text, 10), None);
    }

    #[test]
    fn exact_timings_are_checked_sorted_and_monotonic() {
        let text = "naïve app now";
        let words = [
            WordTiming {
                byte_offset: 11,
                start_ms: 900,
            },
            WordTiming {
                byte_offset: 0,
                start_ms: 10,
            },
            WordTiming {
                byte_offset: 7,
                start_ms: 5,
            },
            WordTiming {
                byte_offset: 3,
                start_ms: 400,
            },
            WordTiming {
                byte_offset: 40,
                start_ms: 50,
            },
        ];
        let timings = exact(text, &words, 800);
        assert_eq!(timings.source, TimingSource::Exact);
        let pairs: Vec<(u32, u32)> = timings
            .words
            .iter()
            .map(|w| (w.byte_offset, w.start_ms))
            .collect();
        // Offset 3 is inside "ï" and 40 is past the end: dropped. 5 ms is lifted to 10 ms; 900
        // is clamped to the audio's 800 ms.
        assert_eq!(pairs, vec![(0, 10), (7, 10), (11, 800)]);
    }

    #[test]
    fn estimates_spread_words_over_the_voiced_audio() {
        // 200 ms of silence, 1 s of voice, 300 ms of silence.
        let mut pcm = vec![0.0; 4_800];
        pcm.extend(std::iter::repeat_n(0.5, 24_000));
        pcm.extend(std::iter::repeat_n(0.0, 7_200));
        let timings = estimate("one two three four", &pcm);
        assert_eq!(timings.source, TimingSource::Estimated);
        assert_eq!(timings.duration_ms, 1_500);
        let starts: Vec<u32> = timings.words.iter().map(|w| w.start_ms).collect();
        // 18 characters over 200..1200 ms: words at characters 0, 4, 8 and 14.
        assert_eq!(starts, vec![200, 422, 644, 977]);
    }

    #[test]
    fn silence_spreads_words_over_the_whole_clause() {
        let timings = estimate("a b", &[0.0; 2_400]);
        let starts: Vec<u32> = timings.words.iter().map(|w| w.start_ms).collect();
        assert_eq!(starts, vec![0, 66]);
    }
}
