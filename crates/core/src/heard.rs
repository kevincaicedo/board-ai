//! The heard prefix (LLD §2.7, invariant I2): exactly what the user heard of one talker stream.
//!
//! A `HeardPrefix` can only be built from a playback position, so no code path can commit text
//! the voice never reached. The position comes from `VoiceStopped`, a playback report, or the
//! end of a clause the runtime reported as played; never from what the talker wrote.

use crate::input::PlaybackPosition;
use crate::limits::HEARD_NOTE_TAIL_BYTES;
use crate::narration::NarrationStream;
use crate::voice::Voice;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeardPrefix {
    /// The speech as written, tags removed.
    caption: String,
    /// The speech with the tags of the ops that were drawn, for the committed assistant message.
    written: String,
    /// The stream was heard to its end.
    complete: bool,
}

impl HeardPrefix {
    /// What the user heard of `narration` (stream `stream` of the turn) when playback stopped at
    /// `position`. `drawn` says whether the stream's op at an index was drawn.
    #[must_use]
    pub fn from_playback(
        position: Option<PlaybackPosition>,
        voice: &Voice,
        stream: usize,
        narration: &NarrationStream,
        drawn: impl Fn(u32) -> bool,
    ) -> Self {
        let total = narration.caption_len();
        let offset = voice.heard_in_stream(position, stream, total);
        let speech = narration.written_prefix(total, |_| false);
        // Whitespace after the last clause is never spoken, so it can't be missed.
        let complete = speech
            .get(offset..)
            .is_none_or(|rest| rest.trim().is_empty());
        Self {
            caption: narration.written_prefix(offset, |_| false),
            written: narration.written_prefix(offset, drawn),
            complete,
        }
    }

    #[must_use]
    pub fn caption(&self) -> &str {
        &self.caption
    }

    #[must_use]
    pub fn written(&self) -> &str {
        &self.written
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.caption.trim().is_empty()
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The note that opens the next user message when the stored reply was longer than what
    /// was heard (case b of LLD §2.7).
    #[must_use]
    pub fn note(&self) -> String {
        let heard = self.caption.trim();
        if heard.is_empty() {
            return "[You were interrupted before the user heard any of your last reply. Nothing \
                    in it was heard.]"
                .to_owned();
        }
        let mut begin = heard.len().saturating_sub(HEARD_NOTE_TAIL_BYTES);
        while !heard.is_char_boundary(begin) {
            begin += 1;
        }
        let ellipsis = if begin > 0 { "…" } else { "" };
        format!(
            "[You were interrupted. The user heard you up to: \"{ellipsis}{}\". Nothing after that \
             was heard.]",
            &heard[begin..]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::HeardPrefix;
    use crate::input::{PlaybackPosition, TimingSource, WordTiming, WordTimings};
    use crate::narration::{ClauseOrigin, NarrationOutput, NarrationStream};
    use crate::voice::{ClauseSource, Voice};

    #[test]
    fn commits_the_heard_words_with_only_the_drawn_tags() {
        let mut narration = NarrationStream::new(0);
        let mut out = Vec::new();
        narration.push(
            r#"Before <node id="pg"/>Postgres it checks <node id="redis"/>Redis first."#,
            &mut out,
        );
        narration.finish(&mut out);
        let mut voice = Voice::default();
        let mut clause_id = None;
        for output in out {
            if let NarrationOutput::Clause(clause) = output {
                let ClauseOrigin::Talker { caption_begin } = clause.origin else {
                    continue;
                };
                let source = ClauseSource::Talker {
                    stream: 0,
                    caption_begin,
                };
                let speakable_len = clause.speakable.text().len();
                let id = voice.add(clause.caption, clause.speakable, source);
                let words = [0_u32, 7, 16, 19, 26]
                    .map(|byte_offset| WordTiming {
                        byte_offset,
                        start_ms: byte_offset * 10,
                    })
                    .to_vec();
                let duration_ms = u32::try_from(speakable_len).expect("short") * 10;
                let timings = WordTimings {
                    source: TimingSource::Estimated,
                    words,
                    duration_ms,
                };
                assert!(voice.ready(id, timings));
                clause_id = Some(id);
            }
        }
        let position = PlaybackPosition {
            clause: clause_id.expect("one clause"),
            offset_ms: 280,
        };
        let heard = HeardPrefix::from_playback(Some(position), &voice, 0, &narration, |op| op == 0);
        assert_eq!(heard.caption(), "Before Postgres it checks Re");
        assert_eq!(
            heard.written(),
            r#"Before <node id="pg"/>Postgres it checks Re"#
        );
        assert!(!heard.is_complete());
        assert_eq!(
            heard.note(),
            "[You were interrupted. The user heard you up to: \"Before Postgres it checks Re\". \
             Nothing after that was heard.]"
        );
        let nothing = HeardPrefix::from_playback(None, &voice, 0, &narration, |_| true);
        assert!(nothing.is_empty());
        assert!(nothing.note().contains("before the user heard any"));
    }
}
