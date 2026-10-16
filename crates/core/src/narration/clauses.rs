//! Cutting speech into clauses and anchoring ops to them (LLD §2.3, §2.4).
//!
//! A clause ends at a sentence end, at a comma once it is [`CLAUSE_COMMA_MIN_BYTES`] long
//! ([`CLAUSE_FIRST_COMMA_MIN_BYTES`] for the stream's first clause, which the user is waiting
//! on), at a newline, or at [`CLAUSE_MAX_BYTES`] (on the last space when there is one). Each
//! boundary needs only the whitespace after it, so a clause is released the moment that
//! whitespace arrives.
//!
//! An op's anchor is the first spoken char after its tag. Ops are held until their clause is cut
//! and then released right after it, so an anchor never names a clause that could still change.

use crate::conversation::cues;
use crate::limits::{CLAUSE_COMMA_MIN_BYTES, CLAUSE_FIRST_COMMA_MIN_BYTES, CLAUSE_MAX_BYTES};
use crate::narration::ops::NarrationOp;
use crate::narration::speakable::Speakable;
use crate::narration::{Clause, ClauseOrigin, NarrationOutput, StreamAnchor, StreamOp};

/// Closing marks that may follow a sentence end ("it fails.)").
const CLOSERS: &[char] = &[')', ']', '"', '\'', '”', '’'];

const SENTENCE_ENDS: &[char] = &['.', '!', '?', '…'];

#[derive(Clone, Debug)]
struct PendingOp {
    index: u32,
    op: NarrationOp,
    /// Byte offset in the open clause, once the first spoken char after the tag has arrived.
    offset: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ClauseCutter {
    open: String,
    open_caption_begin: usize,
    caption_len: usize,
    next_clause: u32,
    last_clause: Option<(u32, usize)>,
    pending: Vec<PendingOp>,
}

impl ClauseCutter {
    /// Bytes of speech (caption text) seen so far in this stream.
    #[must_use]
    pub fn caption_len(&self) -> usize {
        self.caption_len
    }

    pub fn push_op(&mut self, index: u32, op: NarrationOp) {
        self.pending.push(PendingOp {
            index,
            op,
            offset: None,
        });
    }

    pub fn push_char(&mut self, ch: char, out: &mut Vec<NarrationOutput>) {
        // Each cut strictly shortens the open clause, so this ends.
        while self.open.len() + ch.len_utf8() > CLAUSE_MAX_BYTES {
            self.cut_at_cap(out);
        }
        if self.open.is_empty() && ch.is_whitespace() {
            self.caption_len += ch.len_utf8();
            return;
        }
        if self.open.is_empty() {
            self.open_caption_begin = self.caption_len;
        }
        if !ch.is_whitespace() {
            let offset = self.open.len();
            for pending in self.pending.iter_mut().filter(|op| op.offset.is_none()) {
                pending.offset = Some(offset);
            }
        }
        self.open.push(ch);
        self.caption_len += ch.len_utf8();
        if ch.is_whitespace() {
            let before = &self.open[..self.open.len() - ch.len_utf8()];
            let comma_min_bytes = if self.next_clause == 0 {
                CLAUSE_FIRST_COMMA_MIN_BYTES
            } else {
                CLAUSE_COMMA_MIN_BYTES
            };
            let comma = before.ends_with(',') && before.len() >= comma_min_bytes;
            if ch == '\n' || comma || ends_sentence(before) {
                self.cut(out);
            }
        }
    }

    /// Speaks a line of the core's own between clauses (a refusal, a limit notice).
    pub fn inject(&mut self, line: &str, out: &mut Vec<NarrationOutput>) {
        self.cut(out);
        self.emit_clause(line.to_owned(), ClauseOrigin::Core, out);
    }

    /// The stream ended: the open clause is released, and ops with no speech after them anchor
    /// to the end of the last clause.
    pub fn finish(&mut self, out: &mut Vec<NarrationOutput>) {
        self.cut(out);
        let anchor = match self.last_clause {
            Some((clause, caption_len)) => StreamAnchor::Spoken {
                clause,
                caption_offset: caption_len,
            },
            None => StreamAnchor::Unanchored,
        };
        for pending in self.pending.drain(..) {
            debug_assert!(
                pending.offset.is_none(),
                "anchored ops left with their clause"
            );
            out.push(NarrationOutput::Op(StreamOp {
                index: pending.index,
                op: pending.op,
                anchor,
            }));
        }
    }

    /// Releases the whole open clause.
    fn cut(&mut self, out: &mut Vec<NarrationOutput>) {
        let text = std::mem::take(&mut self.open);
        let text = text.trim_end();
        if text.is_empty() {
            return;
        }
        let origin = ClauseOrigin::Talker {
            caption_begin: self.open_caption_begin,
        };
        self.emit_clause(text.to_owned(), origin, out);
    }

    /// The open clause hit the size cap: cut at its last space and carry the rest over.
    fn cut_at_cap(&mut self, out: &mut Vec<NarrationOutput>) {
        let space = self
            .open
            .char_indices()
            .rev()
            .find(|(index, ch)| *index > 0 && ch.is_whitespace())
            .map(|(index, _)| index);
        let Some(space) = space else {
            self.cut(out);
            return;
        };
        let remainder = self.open[space..].trim_start().to_owned();
        let carried_from = self.open.len() - remainder.len();
        let head = self.open[..space].trim_end().to_owned();
        let mut carried = Vec::new();
        for mut pending in std::mem::take(&mut self.pending) {
            match pending.offset {
                Some(offset) if offset >= carried_from => {
                    pending.offset = Some(offset - carried_from);
                    carried.push(pending);
                }
                Some(_) => self.pending.push(pending),
                None => carried.push(pending),
            }
        }
        let origin = ClauseOrigin::Talker {
            caption_begin: self.open_caption_begin,
        };
        self.emit_clause(head, origin, out);
        self.pending = carried;
        self.open_caption_begin += carried_from;
        self.open = remainder;
    }

    /// Emits a clause, then every op anchored in it.
    fn emit_clause(
        &mut self,
        caption: String,
        origin: ClauseOrigin,
        out: &mut Vec<NarrationOutput>,
    ) {
        let index = self.next_clause;
        self.next_clause += 1;
        self.last_clause = Some((index, caption.len()));
        let speakable = Speakable::from_caption(&caption);
        out.push(NarrationOutput::Clause(Clause {
            index,
            caption,
            speakable,
            origin,
        }));
        if origin == ClauseOrigin::Core {
            return;
        }
        let (anchored, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|pending| pending.offset.is_some());
        self.pending = waiting;
        for pending in anchored {
            out.push(NarrationOutput::Op(StreamOp {
                index: pending.index,
                op: pending.op,
                anchor: StreamAnchor::Spoken {
                    clause: index,
                    caption_offset: pending.offset.unwrap_or(0),
                },
            }));
        }
    }
}

fn ends_sentence(text: &str) -> bool {
    let trimmed = text.trim_end_matches(CLOSERS);
    let Some(last) = trimmed.chars().next_back() else {
        return false;
    };
    if !SENTENCE_ENDS.contains(&last) {
        return false;
    }
    let last_word = trimmed
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or(trimmed);
    !(last == '.' && cues::is_abbreviation(last_word))
}

#[cfg(test)]
mod tests {
    use super::ends_sentence;

    #[test]
    fn sentence_ends_skip_abbreviations_and_file_names() {
        assert!(ends_sentence("It checks Redis."));
        assert!(ends_sentence("Why?"));
        assert!(ends_sentence("It fails.)"));
        assert!(!ends_sentence("for example e.g."));
        assert!(!ends_sentence("service"));
        assert!(!ends_sentence(""));
    }
}
