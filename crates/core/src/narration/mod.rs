//! The narration script (LLD §2.3): the talker's streamed text with inline drawing tags, turned
//! into spoken clauses and anchored ops.
//!
//! [`NarrationStream`] is one talker response. It is split-invariant: feeding any split of the
//! input produces the same outputs as feeding it whole (a property test checks every golden file
//! at every split point). It also keeps what was written, so an interruption can commit exactly
//! the heard prefix with the tags of the ops that were drawn (LLD §2.7).

pub mod clauses;
pub mod ops;
pub mod parser;
pub mod speakable;
pub mod tags;

use crate::effect::Dropped;
use crate::limits::{DETOUR_DEPTH_MAX, OPS_PER_RESPONSE_MAX};
use clauses::ClauseCutter;
use ops::NarrationOp;
use parser::{Piece, TagParser};
use speakable::Speakable;

/// Spoken when the talker opens a fourth nested detour (LLD §2.6).
pub const DETOUR_REFUSAL_LINE: &str = "Let's finish this one first.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NarrationOutput {
    /// A clause ready for text-to-speech, released as soon as its boundary was seen.
    Clause(Clause),
    /// An op, released right after the clause it is anchored in.
    Op(StreamOp),
    /// Something dropped at a limit or for being malformed; the speech around it continues.
    Dropped(Dropped),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    /// Position among this stream's clauses, from 0.
    pub index: u32,
    /// The text as written, for captions.
    pub caption: String,
    pub speakable: Speakable,
    pub origin: ClauseOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClauseOrigin {
    /// The talker's words; `caption_begin` is where the clause starts in the stream's speech.
    Talker { caption_begin: usize },
    /// A line the core speaks itself. It is never committed as the talker's words.
    Core,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamOp {
    /// Position among this stream's kept ops, from 0.
    pub index: u32,
    pub op: NarrationOp,
    pub anchor: StreamAnchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamAnchor {
    /// The first spoken char after the tag: a byte offset into the clause's caption. An offset
    /// equal to the caption's length means "when the clause has finished".
    Spoken { clause: u32, caption_offset: usize },
    /// The stream had no speech at all; the op starts when the stream ends.
    Unanchored,
}

/// What the talker wrote, in order, for committing a heard prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Text(String),
    /// A tag as written, with its op's index when it became one.
    Tag {
        written: String,
        op: Option<u32>,
    },
}

#[derive(Clone, Debug)]
pub struct NarrationStream {
    parser: TagParser,
    cutter: ClauseCutter,
    pieces: Vec<Piece>,
    ops_kept: u32,
    planned_depth: usize,
    refused_detours: usize,
    segments: Vec<Segment>,
    written: String,
}

impl NarrationStream {
    /// `planned_depth` is the detour depth once every op already queued has run, so a fourth
    /// detour is refused while it is still being written.
    #[must_use]
    pub fn new(planned_depth: usize) -> Self {
        Self {
            parser: TagParser::new(),
            cutter: ClauseCutter::default(),
            pieces: Vec::new(),
            ops_kept: 0,
            planned_depth,
            refused_detours: 0,
            segments: Vec::new(),
            written: String::new(),
        }
    }

    pub fn push(&mut self, text: &str, out: &mut Vec<NarrationOutput>) {
        self.written.push_str(text);
        let mut pieces = std::mem::take(&mut self.pieces);
        self.parser.push(text, &mut pieces);
        self.consume(&mut pieces, out);
        self.pieces = pieces;
    }

    pub fn finish(&mut self, out: &mut Vec<NarrationOutput>) {
        let mut pieces = std::mem::take(&mut self.pieces);
        self.parser.finish(&mut pieces);
        self.consume(&mut pieces, out);
        self.pieces = pieces;
        self.cutter.finish(out);
    }

    /// Everything the talker wrote, tags included.
    #[must_use]
    pub fn written(&self) -> &str {
        &self.written
    }

    /// Bytes of speech so far (the caption text, tags removed).
    #[must_use]
    pub fn caption_len(&self) -> usize {
        self.cutter.caption_len()
    }

    /// The written text up to `caption_offset` bytes of speech, keeping only the tags whose op
    /// `drawn` says was drawn (LLD §2.7: "text plus the tags of ops that were drawn").
    #[must_use]
    pub fn written_prefix(&self, caption_offset: usize, drawn: impl Fn(u32) -> bool) -> String {
        let mut prefix = String::new();
        let mut caption = 0;
        for segment in &self.segments {
            match segment {
                Segment::Text(text) => {
                    if caption >= caption_offset {
                        break;
                    }
                    let mut take = (caption_offset - caption).min(text.len());
                    while !text.is_char_boundary(take) {
                        take -= 1;
                    }
                    prefix.push_str(&text[..take]);
                    caption += text.len();
                    if take < text.len() {
                        break;
                    }
                }
                Segment::Tag { written, op } => {
                    if op.is_some_and(&drawn) {
                        prefix.push_str(written);
                    }
                }
            }
        }
        prefix
    }

    fn consume(&mut self, pieces: &mut Vec<Piece>, out: &mut Vec<NarrationOutput>) {
        for piece in pieces.drain(..) {
            match piece {
                Piece::Text(text) => {
                    for ch in text.chars() {
                        self.cutter.push_char(ch, out);
                    }
                    self.push_segment_text(&text);
                }
                Piece::Tag(tag) => {
                    let op = match tags::to_op(&tag) {
                        Ok(op) => self.keep_op(op, out),
                        Err(_) => {
                            out.push(NarrationOutput::Dropped(Dropped::NarrationTag));
                            None
                        }
                    };
                    self.segments.push(Segment::Tag {
                        written: tag.written,
                        op,
                    });
                }
                Piece::Dropped(written) => {
                    out.push(NarrationOutput::Dropped(Dropped::NarrationTag));
                    self.segments.push(Segment::Tag { written, op: None });
                }
            }
        }
    }

    fn push_segment_text(&mut self, text: &str) {
        if let Some(Segment::Text(existing)) = self.segments.last_mut() {
            existing.push_str(text);
        } else {
            self.segments.push(Segment::Text(text.to_owned()));
        }
    }

    /// Applies the per-response op cap and the detour depth; returns the kept op's index.
    fn keep_op(&mut self, op: NarrationOp, out: &mut Vec<NarrationOutput>) -> Option<u32> {
        if self.ops_kept as usize >= OPS_PER_RESPONSE_MAX {
            out.push(NarrationOutput::Dropped(Dropped::Op));
            return None;
        }
        match op {
            NarrationOp::Detour { .. } if self.planned_depth >= DETOUR_DEPTH_MAX => {
                out.push(NarrationOutput::Dropped(Dropped::Detour));
                self.refused_detours += 1;
                self.cutter.inject(DETOUR_REFUSAL_LINE, out);
                return None;
            }
            NarrationOp::Detour { .. } => self.planned_depth += 1,
            // The resume that closes a refused detour has nothing to close.
            NarrationOp::Resume if self.refused_detours > 0 => {
                self.refused_detours -= 1;
                return None;
            }
            NarrationOp::Resume => self.planned_depth = self.planned_depth.saturating_sub(1),
            NarrationOp::Plan { .. }
            | NarrationOp::Step { .. }
            | NarrationOp::Node { .. }
            | NarrationOp::Edge { .. }
            | NarrationOp::Annotate { .. }
            | NarrationOp::Strike { .. }
            | NarrationOp::Focus { .. }
            | NarrationOp::Code { .. }
            | NarrationOp::Line { .. }
            | NarrationOp::Trace { .. }
            | NarrationOp::Compare { .. }
            | NarrationOp::Cell { .. }
            | NarrationOp::Close { .. } => {}
        }
        let index = self.ops_kept;
        self.ops_kept += 1;
        self.cutter.push_op(index, op);
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClauseOrigin, DETOUR_REFUSAL_LINE, NarrationOutput, NarrationStream, StreamAnchor,
    };
    use crate::effect::Dropped;
    use crate::limits::{CLAUSE_MAX_BYTES, OPS_PER_RESPONSE_MAX};

    fn run(input: &str, depth: usize) -> Vec<NarrationOutput> {
        let mut stream = NarrationStream::new(depth);
        let mut out = Vec::new();
        stream.push(input, &mut out);
        stream.finish(&mut out);
        out
    }

    fn clauses(out: &[NarrationOutput]) -> Vec<&str> {
        out.iter()
            .filter_map(|output| match output {
                NarrationOutput::Clause(clause) => Some(clause.caption.as_str()),
                NarrationOutput::Op(_) | NarrationOutput::Dropped(_) => None,
            })
            .collect()
    }

    #[test]
    fn cuts_at_sentences_long_commas_and_the_cap() {
        let out = run(
            "Sure. It starts in the browser, then hits the CDN at the edge, and then the load \
             balancer spreads it. Short, clause.",
            0,
        );
        assert_eq!(
            clauses(&out),
            vec![
                "Sure.",
                "It starts in the browser, then hits the CDN at the edge,",
                "and then the load balancer spreads it.",
                "Short, clause."
            ]
        );
        let long = "word ".repeat(100);
        let out = run(&long, 0);
        assert!(
            clauses(&out)
                .iter()
                .all(|clause| clause.len() <= CLAUSE_MAX_BYTES)
        );
        assert_eq!(clauses(&out).join(" "), long.trim_end());
    }

    #[test]
    fn an_op_anchors_on_the_first_spoken_char_after_its_tag() {
        let out = run(
            r#"Then the <node id="lb"/> load balancer. <edge from="a" to="lb"/>"#,
            0,
        );
        let anchors: Vec<StreamAnchor> = out
            .iter()
            .filter_map(|output| match output {
                NarrationOutput::Op(op) => Some(op.anchor),
                NarrationOutput::Clause(_) | NarrationOutput::Dropped(_) => None,
            })
            .collect();
        assert_eq!(
            anchors,
            vec![
                StreamAnchor::Spoken {
                    clause: 0,
                    caption_offset: "Then the  ".len()
                },
                StreamAnchor::Spoken {
                    clause: 0,
                    caption_offset: "Then the  load balancer.".len()
                },
            ]
        );
        assert_eq!(run(r#"<node id="a"/>"#, 0).len(), 1);
    }

    #[test]
    fn a_fourth_detour_is_refused_out_loud_and_its_resume_ignored() {
        let out = run(
            r#"<detour title="ttl"/>A TTL is a timer. <resume/>Back."#,
            3,
        );
        assert!(out.contains(&NarrationOutput::Dropped(Dropped::Detour)));
        let core: Vec<&str> = out
            .iter()
            .filter_map(|output| match output {
                NarrationOutput::Clause(clause) if clause.origin == ClauseOrigin::Core => {
                    Some(clause.caption.as_str())
                }
                NarrationOutput::Clause(_)
                | NarrationOutput::Op(_)
                | NarrationOutput::Dropped(_) => None,
            })
            .collect();
        assert_eq!(core, vec![DETOUR_REFUSAL_LINE]);
        assert!(
            !out.iter()
                .any(|output| matches!(output, NarrationOutput::Op(_)))
        );
    }

    #[test]
    fn ops_past_the_cap_are_dropped() {
        let input = r#"<focus target="a"/>"#.repeat(OPS_PER_RESPONSE_MAX + 3);
        let out = run(&format!("{input} done."), 0);
        let kept = out
            .iter()
            .filter(|output| matches!(output, NarrationOutput::Op(_)))
            .count();
        let dropped = out
            .iter()
            .filter(|output| **output == NarrationOutput::Dropped(Dropped::Op))
            .count();
        assert_eq!((kept, dropped), (OPS_PER_RESPONSE_MAX, 3));
    }

    #[test]
    fn the_written_prefix_keeps_only_drawn_tags() {
        let mut stream = NarrationStream::new(0);
        let mut out = Vec::new();
        stream.push(
            r#"It reaches <node id="lb"/>the balancer, then <node id="api"/>three pods."#,
            &mut out,
        );
        stream.finish(&mut out);
        let heard = "It reaches the balancer, then thr".len();
        assert_eq!(
            stream.written_prefix(heard, |op| op == 0),
            r#"It reaches <node id="lb"/>the balancer, then thr"#
        );
    }
}
