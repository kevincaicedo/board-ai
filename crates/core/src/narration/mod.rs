//! The narration script (LLD §2.3): the talker's streamed text with inline drawing tags, turned
//! into spoken clauses and anchored ops.
//!
//! [`NarrationStream`] is one talker response. It is split-invariant: feeding any split of the
//! input produces the same outputs as feeding it whole (a property test checks every golden file
//! at every split point). It also keeps what was written, so an interruption can commit exactly
//! the heard prefix with the tags of the ops that were drawn (LLD §2.7).

mod block;
pub mod citation;
pub mod clauses;
pub mod diagram;
pub mod ops;
pub mod parser;
pub mod quality;
pub mod speakable;
pub mod spoken_calls;
pub mod tags;

use crate::effect::Dropped;
use crate::limits::{CITATION_TOKEN_MAX_BYTES, DETOUR_DEPTH_MAX, OPS_PER_RESPONSE_MAX};
use block::{Ending, OpenBlock};
use clauses::ClauseCutter;
use ops::NarrationOp;
use parser::{Piece, RawTag, TagParser};
use speakable::Speakable;
use spoken_calls::{SpokenCall, SpokenCalls, without_stray_tokens};

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
    /// The spoken word being read, held back from the clause cutter until it ends, so a
    /// `path:line` citation anchors its source chip on its own first character.
    word: Word,
    /// Tool calls written as words, kept out of speech (D-179).
    calls: SpokenCalls,
    /// A `<diagram>` or `<detail>` block being read (D-187).
    block: Option<OpenBlock>,
    /// The diagrams this stream staged, by op index, and whether any drawing op came after
    /// each: one nothing drew after is revealed whole by the engine.
    staged: Vec<(u32, bool)>,
}

/// The word being read: held while it could still be a citation, passed through once it is too
/// long to be one.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Word {
    Held(String),
    Overflowed,
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
            word: Word::Held(String::new()),
            calls: SpokenCalls::default(),
            block: None,
            staged: Vec::new(),
        }
    }

    /// The diagrams this stream staged that no drawing tag after them reveals, by op index.
    pub fn unrevealed_diagrams(&self) -> impl Iterator<Item = u32> + '_ {
        self.staged
            .iter()
            .filter(|(_, revealed)| !revealed)
            .map(|(op, _)| *op)
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
        self.end_block(Ending::Unclosed, "", out);
        self.end_word(out);
        self.boundary(out);
        self.cutter.finish(out);
    }

    /// Tool calls the talker wrote as words, which were kept out of speech (D-179).
    #[must_use]
    pub fn spoken_calls(&self) -> &[SpokenCall] {
        self.calls.calls()
    }

    /// Repo reads the talker's speech announced ("I'll list the files"), each tool once, in
    /// the order announced (D-232).
    #[must_use]
    pub fn announced_reads(&self) -> &[&'static str] {
        self.calls.announced()
    }

    /// Everything the talker wrote, tags included.
    #[must_use]
    pub fn written(&self) -> &str {
        &self.written
    }

    /// Bytes of speech so far (the caption text, tags removed), the word being read included.
    #[must_use]
    pub fn caption_len(&self) -> usize {
        let held = match &self.word {
            Word::Held(word) => word.len(),
            Word::Overflowed => 0,
        };
        self.cutter.caption_len() + held
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
                        self.text_char(ch, out);
                    }
                }
                Piece::Tag(tag) => self.tag(tag, out),
                Piece::Dropped(written) => {
                    out.push(NarrationOutput::Dropped(Dropped::NarrationTag));
                    self.dropped_tag(written, out);
                }
                Piece::Close(name) => {
                    let closing = format!("</{name}>");
                    if self.block.as_ref().is_some_and(|block| block.closes(&name)) {
                        self.end_block(Ending::Closed, &closing, out);
                        continue;
                    }
                    // A closing tag of no open block is dropped, as any closing tag was.
                    out.push(NarrationOutput::Dropped(Dropped::NarrationTag));
                    self.dropped_tag(closing, out);
                }
            }
        }
    }

    /// A tag: a part of the open block, the start of a block, or an op. Any tag other than a
    /// part ends the open block first.
    fn tag(&mut self, tag: RawTag, out: &mut Vec<NarrationOutput>) {
        if let Some(block) = &mut self.block {
            if OpenBlock::takes(&tag) {
                if let Some(what) = block.part(&tag) {
                    out.push(NarrationOutput::Dropped(what));
                }
                return;
            }
            self.end_block(Ending::Unclosed, "", out);
        }
        self.end_word(out);
        self.boundary(out);
        let op = match OpenBlock::open(&tag) {
            Some(Ok(block)) => {
                self.block = Some(block);
                return;
            }
            Some(Err(_)) => None,
            None => tags::to_op(&tag).ok(),
        };
        let op = match op {
            Some(op) => self.keep_op(op, out),
            None => {
                out.push(NarrationOutput::Dropped(Dropped::NarrationTag));
                None
            }
        };
        self.segments.push(Segment::Tag {
            written: tag.written,
            op,
        });
    }

    /// A dropped tag's text: kept with the open block, or among what was written.
    fn dropped_tag(&mut self, written: String, out: &mut Vec<NarrationOutput>) {
        if let Some(block) = &mut self.block {
            block.malformed(&written);
            return;
        }
        self.end_word(out);
        self.boundary(out);
        self.segments.push(Segment::Tag { written, op: None });
    }

    /// Text: inside a block it is the block's, until it can only be speech; outside, speech.
    fn text_char(&mut self, ch: char, out: &mut Vec<NarrationOutput>) {
        let Some(block) = &mut self.block else {
            self.speak_char(ch, out);
            return;
        };
        if block.text(ch) {
            self.end_block(Ending::Unclosed, "", out);
        }
    }

    /// The open block ends: its op is kept (anchored on the first word after it, like any tag),
    /// and, when it ended unclosed, the text since its last part is spoken.
    fn end_block(&mut self, ending: Ending, closing: &str, out: &mut Vec<NarrationOutput>) {
        let Some(block) = self.block.take() else {
            return;
        };
        let ended = block.end(ending, closing);
        let op = match ended.op {
            Some(op) => self.keep_op(op, out),
            None => {
                out.push(NarrationOutput::Dropped(Dropped::NarrationTag));
                None
            }
        };
        self.segments.push(Segment::Tag {
            written: ended.written,
            op,
        });
        for ch in ended.speech.chars() {
            self.speak_char(ch, out);
        }
    }

    /// Whitespace ends the word being read; anything else joins it, or goes straight through
    /// once the word is too long to be a citation (or a call's argument).
    fn speak_char(&mut self, ch: char, out: &mut Vec<NarrationOutput>) {
        if ch.is_whitespace() {
            self.end_word(out);
            let said = self.calls.whitespace(ch);
            self.say(&said, out);
            return;
        }
        match &mut self.word {
            Word::Held(word) if word.len() + ch.len_utf8() <= CITATION_TOKEN_MAX_BYTES => {
                word.push(ch);
            }
            Word::Held(word) => {
                let word = std::mem::take(word);
                self.word = Word::Overflowed;
                self.boundary(out);
                self.say(&word, out);
                self.say_char(ch, out);
            }
            Word::Overflowed => self.say_char(ch, out),
        }
    }

    /// The word ended (whitespace, a tag, the end of the stream). gemma4's stray channel tokens
    /// are taken out of it, a tool call written as words is kept out of speech, a citation
    /// becomes an op anchored on the word's first character, then the word goes to the clause
    /// cutter.
    fn end_word(&mut self, out: &mut Vec<NarrationOutput>) {
        let word = match std::mem::replace(&mut self.word, Word::Held(String::new())) {
            Word::Held(word) if !word.is_empty() => without_stray_tokens(word),
            Word::Held(_) | Word::Overflowed => return,
        };
        if word.is_empty() {
            return;
        }
        let fate = self.calls.word(&word);
        self.say(&fate.released, out);
        if !fate.speak {
            return;
        }
        if let Some((path, line)) = citation::parse(&word) {
            self.keep_op(NarrationOp::Cite { path, line }, out);
        }
        self.say(&word, out);
    }

    /// A tag, an overlong word or the end of the stream ends a tool call written as words; a
    /// tool's name held to see what followed it is spoken after all.
    fn boundary(&mut self, out: &mut Vec<NarrationOutput>) {
        let released = self.calls.boundary();
        self.say(&released, out);
    }

    fn say(&mut self, text: &str, out: &mut Vec<NarrationOutput>) {
        for ch in text.chars() {
            self.say_char(ch, out);
        }
    }

    /// One character of speech: to the clause cutter, and to what was written as speech, which
    /// the heard prefix is measured against (LLD §2.7).
    fn say_char(&mut self, ch: char, out: &mut Vec<NarrationOutput>) {
        self.cutter.push_char(ch, out);
        if let Some(Segment::Text(existing)) = self.segments.last_mut() {
            existing.push(ch);
        } else {
            self.segments.push(Segment::Text(ch.to_string()));
        }
    }

    /// Applies the per-response op cap and the detour depth; returns the kept op's index.
    fn keep_op(&mut self, op: NarrationOp, out: &mut Vec<NarrationOutput>) -> Option<u32> {
        if self.ops_kept as usize >= OPS_PER_RESPONSE_MAX {
            out.push(NarrationOutput::Dropped(Dropped::Op));
            return None;
        }
        match op {
            NarrationOp::Detour { .. } | NarrationOp::Detail { .. }
                if self.planned_depth >= DETOUR_DEPTH_MAX =>
            {
                out.push(NarrationOutput::Dropped(Dropped::Detour));
                self.refused_detours += 1;
                self.cutter.inject(DETOUR_REFUSAL_LINE, out);
                return None;
            }
            NarrationOp::Detour { .. } | NarrationOp::Detail { .. } => self.planned_depth += 1,
            // The resume that closes a refused detour has nothing to close.
            NarrationOp::Resume if self.refused_detours > 0 => {
                self.refused_detours -= 1;
                return None;
            }
            NarrationOp::Resume => self.planned_depth = self.planned_depth.saturating_sub(1),
            // A page turn closes every open detour first (D-177).
            NarrationOp::Page { .. } => {
                self.planned_depth = 0;
                self.refused_detours = 0;
            }
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
            | NarrationOp::Close { .. }
            | NarrationOp::MarkCode { .. }
            | NarrationOp::Recap { .. }
            | NarrationOp::Participant { .. }
            | NarrationOp::Message { .. }
            | NarrationOp::Cells(_)
            | NarrationOp::Write { .. }
            | NarrationOp::Cite { .. }
            | NarrationOp::Diagram(_)
            | NarrationOp::Reveal { .. } => {}
        }
        let index = self.ops_kept;
        self.ops_kept += 1;
        if op.draws() {
            for (_, revealed) in &mut self.staged {
                *revealed = true;
            }
        }
        let stages = matches!(
            op,
            NarrationOp::Diagram(_) | NarrationOp::Detail { spec: Some(_), .. }
        );
        if stages {
            self.staged.push((index, false));
        }
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

    /// The first clause, which the user waits on, ends at a comma from 16 bytes; the rest keep
    /// the 40-byte minimum.
    #[test]
    fn the_first_clause_ends_at_an_earlier_comma() {
        let out = run(
            "Good question, it starts at the browser, then the API checks Redis, and returns.",
            0,
        );
        assert_eq!(
            clauses(&out),
            vec![
                "Good question, it starts at the browser,",
                "then the API checks Redis, and returns."
            ]
        );
        assert_eq!(
            clauses(&run("A request starts at the client, then the API.", 0)),
            vec!["A request starts at the client,", "then the API."]
        );
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
