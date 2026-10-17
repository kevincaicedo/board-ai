//! A `<diagram>` or `<detail>` block being read (D-187, D-188). The block is a whole diagram in
//! the narration vocabulary: its tags are collected into a spec, not drawn as they come, and its
//! text is not speech. It ends at its closing tag. A talker that forgets the closing tag goes on
//! speaking or tagging the narration, so the block also ends at the first tag that isn't one of
//! a diagram's parts (a `<reveal>`), at a stretch of text too long to be anything but speech, or
//! at the end of the stream; then the text since its last part is speech after all.
//!
//! Everything is decided a piece, or a character, at a time, so a block is read the same however
//! the stream is split.

use crate::effect::Dropped;
use crate::ids::ElementId;
use crate::limits::{DIAGRAM_BLOCK_MAX_BYTES, DIAGRAM_GAP_TEXT_MAX_BYTES, DIAGRAM_PARTS_MAX};
use crate::narration::diagram::DiagramSpec;
use crate::narration::ops::NarrationOp;
use crate::narration::parser::RawTag;
use crate::narration::tags::{BadTag, diagram};

/// The tags a block collects as the diagram's parts.
const PART_TAG_NAMES: &[&str] = &["node", "edge", "cells", "record", "note"];

/// Drawing tags a staged diagram can't hold (a sequence diagram's): inside a block they are
/// dropped and counted rather than drawn at once. Any other tag is narration and ends the block.
const MISPLACED_TAG_NAMES: &[&str] = &["participant", "message", "write", "annotate", "strike"];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Diagram,
    Detail {
        of: ElementId,
        title: Option<String>,
    },
}

/// How a block ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// At its closing tag: the text since its last part was inside the block.
    Closed,
    /// Without one: the text since its last part is speech.
    Unclosed,
}

/// A block that ended: its op (none when it held nothing to stage), what was written of it, and
/// the speech that followed its last part when it ended unclosed.
pub(super) struct Ended {
    pub op: Option<NarrationOp>,
    pub written: String,
    pub speech: String,
}

#[derive(Clone, Debug)]
pub(super) struct OpenBlock {
    kind: Kind,
    spec: DiagramSpec,
    written: String,
    /// The text since the last tag, and how many of its bytes aren't whitespace.
    gap: String,
    gap_text_bytes: usize,
}

impl OpenBlock {
    /// The block `tag` opens, if it opens one: `<diagram …>` or `<detail of="…">` written
    /// without a closing slash.
    pub(super) fn open(tag: &RawTag) -> Option<Result<Self, BadTag>> {
        if tag.self_closing {
            return None;
        }
        let kind = match tag.name.as_str() {
            "diagram" => Kind::Diagram,
            "detail" => match diagram::detail_target(tag) {
                Ok((of, title)) => Kind::Detail { of, title },
                Err(bad) => return Some(Err(bad)),
            },
            _ => return None,
        };
        let mut spec = diagram::open(tag);
        if let Kind::Detail { of, title } = &kind {
            // A detail's diagram is named after what it drills into, unless its tag says.
            spec.title = title.clone().unwrap_or_else(|| of.as_str().to_owned());
        }
        Some(Ok(Self {
            kind,
            spec,
            written: tag.written.clone(),
            gap: String::new(),
            gap_text_bytes: 0,
        }))
    }

    /// `</name>` ends this block.
    pub(super) fn closes(&self, name: &str) -> bool {
        match self.kind {
            Kind::Diagram => name == "diagram",
            Kind::Detail { .. } => name == "detail",
        }
    }

    /// Whether `tag` belongs inside the block (a part, or a drawing tag it can't hold, which is
    /// dropped); any other tag ends the block first.
    pub(super) fn takes(tag: &RawTag) -> bool {
        PART_TAG_NAMES.contains(&tag.name.as_str())
            || diagram::PART_KIND_TAG_NAMES.contains(&tag.name.as_str())
            || MISPLACED_TAG_NAMES.contains(&tag.name.as_str())
    }

    /// A part's tag. Returns what was dropped, if it was.
    pub(super) fn part(&mut self, tag: &RawTag) -> Option<Dropped> {
        self.end_gap();
        if !self.room(&tag.written) {
            return Some(self.drop_part(Dropped::DiagramPart));
        }
        self.written.push_str(&tag.written);
        let misplaced = MISPLACED_TAG_NAMES.contains(&tag.name.as_str());
        if misplaced || self.spec.items.len() >= DIAGRAM_PARTS_MAX {
            return Some(self.drop_part(Dropped::DiagramPart));
        }
        match diagram::part(tag) {
            Ok(item) => {
                self.spec.items.push(item);
                None
            }
            Err(_) => Some(self.drop_part(Dropped::NarrationTag)),
        }
    }

    /// A malformed tag inside the block, as written.
    pub(super) fn malformed(&mut self, written: &str) {
        self.end_gap();
        if self.room(written) {
            self.written.push_str(written);
        }
        self.spec.dropped += 1;
    }

    /// One character of text inside the block. True when the text since the last part can only
    /// be speech: the block ends unclosed there.
    pub(super) fn text(&mut self, ch: char) -> bool {
        self.gap.push(ch);
        if !ch.is_whitespace() {
            self.gap_text_bytes += ch.len_utf8();
        }
        self.gap_text_bytes > DIAGRAM_GAP_TEXT_MAX_BYTES
    }

    /// The block ends.
    pub(super) fn end(mut self, ending: Ending, closing: &str) -> Ended {
        let speech = match ending {
            Ending::Closed => {
                self.end_gap();
                if self.room(closing) {
                    self.written.push_str(closing);
                }
                String::new()
            }
            Ending::Unclosed => std::mem::take(&mut self.gap),
        };
        let spec = (!self.spec.items.is_empty()).then(|| Box::new(self.spec));
        let op = match self.kind {
            Kind::Diagram => spec.map(NarrationOp::Diagram),
            Kind::Detail { of, title } => Some(NarrationOp::Detail { of, title, spec }),
        };
        Ended {
            op,
            written: self.written,
            speech,
        }
    }

    /// The text before a tag was inside the block.
    fn end_gap(&mut self) {
        let gap = std::mem::take(&mut self.gap);
        if self.room(&gap) {
            self.written.push_str(&gap);
        }
        self.gap_text_bytes = 0;
    }

    fn room(&self, more: &str) -> bool {
        self.written.len() + more.len() <= DIAGRAM_BLOCK_MAX_BYTES
    }

    fn drop_part(&mut self, what: Dropped) -> Dropped {
        self.spec.dropped += 1;
        what
    }
}
