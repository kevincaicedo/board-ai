//! The assistant's side of a turn: talker requests and streams, tools, and harness results
//! (LLD §2.1, §3.3, §3.5).
//!
//! A turn may take several streams: the talker stops with `ToolUse`, the results come back, and
//! the next request continues the same spoken reply. At most one stream is in flight (I5), and a
//! `CancelTalk` always precedes a new `Talk`.

mod context;
mod tools;

use std::collections::BTreeSet;

use crate::board::summary::summary;
use crate::conversation::detail_cues::detail_cue;
use crate::conversation::draw_cues::{asks_for_drawing, asks_for_map};
use crate::effect::{Dropped, Effect, LogRecord, Milestone, TalkRequest};
use crate::engine::nudge::shows;
use crate::engine::ops::{Anchor, QueuedOp};
use crate::engine::text::{CALL_NOT_RUN, REFUSAL_LINE, failure_line};
use crate::engine::{Core, milestone};
use crate::history::{AssistantPart, Message, RawAssistant, UserPart};
use crate::ids::{ClauseId, OpId, ToolCallId};
use crate::input::StopReason;
use crate::limits::RESPONSE_TEXT_MAX_BYTES;
use crate::narration::speakable::Speakable;
use crate::narration::{ClauseOrigin, NarrationOutput, NarrationStream, StreamAnchor};
use crate::protocol::UiEvent;
use crate::time::SessionTime;
use crate::tools::ToolInvocation;
use crate::voice::ClauseSource;
use tools::error_result;

/// One talker stream: its narration, its tool calls, and whether it is in the history yet.
#[derive(Clone, Debug)]
pub(super) struct Stream {
    pub narration: NarrationStream,
    /// The `OpId` of this stream's op 0.
    pub op_base: u32,
    /// Clause ids, by the stream's clause index.
    clauses: Vec<ClauseId>,
    calls: Vec<ToolInvocation>,
    stop: Option<StopReason>,
    raw: Option<RawAssistant>,
    pub committed: bool,
    dropped_ops: u32,
}

impl Stream {
    pub(super) fn has_stopped(&self) -> bool {
        self.stop.is_some()
    }

    /// The talker called a tool in this stream.
    pub(super) fn has_calls(&self) -> bool {
        !self.calls.is_empty()
    }

    /// The stream ended as the talker meant it to: its turn's end, a tool round or its length
    /// limit, not a refusal or a failure.
    pub(super) fn ended_normally(&self) -> bool {
        matches!(
            self.stop,
            Some(StopReason::EndTurn | StopReason::ToolUse | StopReason::MaxTokens)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ToolSlot {
    id: ToolCallId,
    result: Option<UserPart>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Reply {
    pub streams: Vec<Stream>,
    pub in_flight: bool,
    /// The last stream's tool calls in call order, with results as they arrive.
    pub tools: Vec<ToolSlot>,
    rounds: usize,
    /// The talker has nothing more to say this turn.
    pub done: bool,
    /// The `path:line` citations this reply showed a source chip for.
    pub cited: BTreeSet<(String, u32)>,
    /// The turn's words asked for a drawing (D-178).
    pub asks_drawing: bool,
    /// The turn's words asked for a map, a walkthrough of a whole thing, or more detail on a part
    /// (D-239, D-247).
    pub asks_map: bool,
    /// The reply drew something, or showed a card or a view.
    pub shown: bool,
    /// How many times the core asked the talker once more this turn (D-178).
    pub nudges: usize,
    /// A repo read of this reply returned (a listing, a file or search hits): what a map or a
    /// tour draws can come from it (D-249).
    pub read_repo: bool,
    /// How many times this turn the core asked for the drawing after reads (D-249).
    pub read_draw_nudges: usize,
    /// The parts its diagrams still held when everything was said were queued (D-237, D-250).
    pub leftovers_queued: bool,
    /// The reply called a repo read, or wrote one instead of calling it: a tour, whose module
    /// names are checked (D-180).
    pub touring: bool,
}

impl Reply {
    /// A reply the core speaks itself, with no talker stream: done from the start, so the turn
    /// ends once its line has played.
    pub(super) fn spoken_by_core() -> Self {
        Self {
            done: true,
            ..Self::default()
        }
    }
}

pub(super) use context::ContextTracker;

impl Core {
    /// Sends the talker the whole history plus the pending user message. Board, thread and
    /// notices that changed go first, as a system message (history is append-only).
    pub(super) fn request_talk(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        // No reply yet: a new turn (the user's next question, or a harness report), so the
        // topic has moved on (D-148). A tool round continues the same reply.
        let new_turn = self.reply.is_none();
        if new_turn {
            self.recap_leaves(effects);
        }
        debug_assert!(
            !self.is_drawing(),
            "I14: no talker request while the user draws"
        );
        let planned_depth = self.queue.planned_depth(self.thread.depth());
        if self.reply.as_ref().is_some_and(|reply| reply.in_flight) {
            effects.push(Effect::CancelTalk);
        }
        // Before the context: the board summary and a note name what a brief staged (D-242).
        self.stage_brief_diagrams(new_turn, effects);
        self.append_context(now);
        let mut messages = self.conversation.messages().to_vec();
        if let Some(pending) = &self.user.pending {
            messages.push(pending.message.clone());
        }
        effects.push(Effect::Talk {
            request: TalkRequest { messages },
        });
        milestone(effects, Milestone::TalkerRequested);
        self.marks.first_token = false;
        let op_base = self.queue.next_op;
        let words = self
            .user
            .pending
            .as_ref()
            .filter(|_| new_turn)
            .map(|pending| pending.text.as_str());
        let asks_drawing = words.is_some_and(asks_for_drawing);
        let asks_map =
            words.is_some_and(|words| asks_for_map(words) || detail_cue(words).is_some());
        let reply = self.reply.get_or_insert_with(Reply::default);
        reply.asks_drawing |= asks_drawing;
        reply.asks_map |= asks_map;
        reply.in_flight = true;
        reply.done = false;
        reply.streams.push(Stream {
            narration: NarrationStream::new(planned_depth),
            op_base,
            clauses: Vec::new(),
            calls: Vec::new(),
            stop: None,
            raw: None,
            committed: false,
            dropped_ops: 0,
        });
    }

    fn append_context(&mut self, now: SessionTime) {
        self.board.settle(now);
        let board = summary(&self.board);
        let thread = self.thread.describe();
        let mut sections = Vec::new();
        if board != self.context.board {
            sections.push(format!("Board now:\n{board}"));
            self.context.board = board;
        }
        if thread != self.context.thread {
            sections.push(format!("Thread: {thread}"));
            self.context.thread = thread;
        }
        sections.extend(self.context.notices.drain(..));
        if !sections.is_empty() {
            self.conversation.push(Message::System {
                text: sections.join("\n\n"),
            });
        }
        // Harness output can quote repo text: it never gets a system message's authority. It
        // goes in a user turn, fenced as untrusted data (LLD §9). Quiet reports (a prefetched
        // brief) go with a new turn only: in a tool round they would come between a call's
        // result and the reply that uses it.
        let quiet: Vec<String> = if self.reply.is_none() {
            self.context.quiet.drain(..).collect()
        } else {
            Vec::new()
        };
        for report in quiet.into_iter().chain(self.context.reports.drain(..)) {
            self.conversation.push(Message::User {
                parts: vec![UserPart::Text { text: report }],
            });
        }
    }

    pub(super) fn on_talker_delta(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) {
        let Some(reply) = self.reply.as_mut().filter(|reply| reply.in_flight) else {
            return;
        };
        let index = reply.streams.len() - 1;
        let written = reply.streams[index].narration.written().len();
        let mut room = RESPONSE_TEXT_MAX_BYTES
            .saturating_sub(written)
            .min(text.len());
        while !text.is_char_boundary(room) {
            room -= 1;
        }
        let mut out = Vec::new();
        reply.streams[index].narration.push(&text[..room], &mut out);
        if !self.marks.first_token {
            self.marks.first_token = true;
            milestone(effects, Milestone::TalkerFirstToken);
        }
        self.take_outputs(index, out, effects);
        if room < text.len() {
            // Past the response limit: stop the stream and finish the current sentence.
            effects.push(Effect::CancelTalk);
            self.end_stream(now, StopReason::MaxTokens, None, effects);
        }
    }

    pub(super) fn on_talker_done(
        &mut self,
        now: SessionTime,
        stop: StopReason,
        raw: Option<RawAssistant>,
        effects: &mut Vec<Effect>,
    ) {
        if self.reply.as_ref().is_some_and(|reply| reply.in_flight) {
            self.end_stream(now, stop, raw, effects);
        }
    }

    fn end_stream(
        &mut self,
        now: SessionTime,
        stop: StopReason,
        raw: Option<RawAssistant>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(reply) = self.reply.as_mut() else {
            return;
        };
        reply.in_flight = false;
        let index = reply.streams.len() - 1;
        let stream = &mut reply.streams[index];
        let mut out = Vec::new();
        stream.narration.finish(&mut out);
        stream.stop = Some(stop.clone());
        stream.raw = raw;
        let has_calls = !stream.calls.is_empty();
        self.take_outputs(index, out, effects);
        self.notice_dropped_ops(index);
        if !matches!(stop, StopReason::Refusal | StopReason::Failed { .. }) {
            self.carry_out_written_calls(index, effects);
            self.notice_spoken_calls(index);
            self.mark_tour(index);
            self.auto_reveal(index);
        }
        // A proposed change is asked about only when the reply ends normally; a refusal or a
        // failure ends the reply with a problem line, and the proposal is dropped with it.
        match stop {
            StopReason::ToolUse if has_calls => self.maybe_tool_round(now, effects),
            StopReason::EndTurn | StopReason::ToolUse => {
                self.finish_stream(index);
                if !self.maybe_nudge(now, index, effects) {
                    self.ask_change(effects);
                }
            }
            StopReason::MaxTokens => {
                self.finish_stream(index);
                self.ask_change(effects);
                self.context.notice(
                    "Your last reply hit its length limit and stopped at the end of its last \
                     clause; that is where the user stopped hearing it."
                        .to_owned(),
                );
            }
            StopReason::Refusal => {
                self.finish_stream(index);
                self.speak_problem(REFUSAL_LINE, "The model declined to answer.", effects);
            }
            StopReason::Failed { failure } => {
                self.finish_stream(index);
                let line = failure_line(&failure, &self.config.talker_name);
                self.context.notice(format!(
                    "Your last request failed: {}. The user heard: \"{line}\"",
                    failure.detail
                ));
                self.speak_problem(&line, &failure.detail, effects);
            }
        }
    }

    /// The stream ended the turn: store it when the provider gave its content (a stream without
    /// it is committed later as what was heard), and mark the reply done.
    fn finish_stream(&mut self, index: usize) {
        self.commit_stream(index);
        if let Some(reply) = self.reply.as_mut() {
            reply.done = true;
        }
    }

    /// Commits stream `index`, and right after it a result for each of its calls: the answer
    /// that came in, or an error for a call it stopped before using. A `tool_use` without its
    /// result in the next message makes every later request invalid, and the history can't be
    /// edited (review finding 1).
    fn commit_stream(&mut self, index: usize) {
        let Some(stream) = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
        else {
            return;
        };
        if stream.committed || (stream.raw.is_none() && stream.calls.is_empty()) {
            return;
        }
        let calls: Vec<ToolCallId> = stream.calls.iter().map(|call| call.id.clone()).collect();
        let mut parts = Vec::new();
        if !stream.narration.written().is_empty() {
            parts.push(AssistantPart::Text {
                text: stream.narration.written().to_owned(),
            });
        }
        parts.extend(stream.calls.iter().map(|call| AssistantPart::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input_json: call.input_json.clone(),
        }));
        let message = Message::Assistant {
            parts,
            raw: stream.raw.clone(),
        };
        self.flush_user();
        self.conversation.push(message);
        if let Some(stream) = self
            .reply
            .as_mut()
            .and_then(|reply| reply.streams.get_mut(index))
        {
            stream.committed = true;
        }
        if !calls.is_empty() {
            let results = self.take_results(&calls);
            self.conversation.push(Message::User { parts: results });
        }
    }

    /// The results for `calls`, in call order. Answers still on their way are ignored when they
    /// arrive: their slots are gone.
    fn take_results(&mut self, calls: &[ToolCallId]) -> Vec<UserPart> {
        let mut slots = self
            .reply
            .as_mut()
            .map(|reply| std::mem::take(&mut reply.tools))
            .unwrap_or_default();
        calls
            .iter()
            .map(|id| {
                slots
                    .iter_mut()
                    .find(|slot| slot.id == *id && slot.result.is_some())
                    .and_then(|slot| slot.result.take())
                    .unwrap_or_else(|| error_result(id, CALL_NOT_RUN.to_owned()))
            })
            .collect()
    }

    fn notice_dropped_ops(&mut self, index: usize) {
        let dropped = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
            .map_or(0, |stream| stream.dropped_ops);
        if dropped > 0 {
            self.context.notice(format!(
                "{dropped} drawing tags in your last reply were over the limit of 200 per reply \
                 and were not drawn."
            ));
        }
    }

    /// A line the core says itself, with the problem shown on screen (never silence).
    fn speak_problem(&mut self, line: &str, detail: &str, effects: &mut Vec<Effect>) {
        let clause = self.voice.add(
            line.to_owned(),
            Speakable::from_caption(line),
            ClauseSource::Core,
        );
        effects.push(Effect::Synthesize {
            clause,
            text: line.to_owned(),
        });
        effects.push(Effect::Ui(UiEvent::Problem {
            spoken: line.to_owned(),
            detail: detail.to_owned(),
        }));
    }

    /// Clauses go to text-to-speech at once; ops wait in the queue for their words.
    fn take_outputs(
        &mut self,
        index: usize,
        outputs: Vec<NarrationOutput>,
        effects: &mut Vec<Effect>,
    ) {
        for output in outputs {
            match output {
                NarrationOutput::Clause(clause) => {
                    let source = match clause.origin {
                        ClauseOrigin::Talker { caption_begin } => ClauseSource::Talker {
                            stream: index,
                            caption_begin,
                        },
                        ClauseOrigin::Core => ClauseSource::Core,
                    };
                    let text = clause.speakable.text().to_owned();
                    let talker = matches!(source, ClauseSource::Talker { .. });
                    let id = self.voice.add(clause.caption, clause.speakable, source);
                    effects.push(Effect::Synthesize { clause: id, text });
                    if talker {
                        // A page citation in it waits for the voice (D-257).
                        self.follow_clause(id);
                    }
                    if let Some(stream) = self.stream_mut(index) {
                        stream.clauses.push(id);
                    }
                }
                NarrationOutput::Op(op) => {
                    if shows(&op.op)
                        && let Some(reply) = self.reply.as_mut()
                    {
                        reply.shown = true;
                    }
                    self.queue_op(index, op);
                }
                NarrationOutput::Dropped(what) => {
                    effects.push(Effect::Log(LogRecord::Dropped { what, count: 1 }));
                    if what == Dropped::Op
                        && let Some(stream) = self.stream_mut(index)
                    {
                        stream.dropped_ops += 1;
                    }
                }
            }
        }
    }

    fn stream_mut(&mut self, index: usize) -> Option<&mut Stream> {
        self.reply.as_mut()?.streams.get_mut(index)
    }

    fn queue_op(&mut self, index: usize, op: crate::narration::StreamOp) {
        let Some(stream) = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
        else {
            return;
        };
        let id = OpId(stream.op_base.saturating_add(op.index));
        let anchor = match op.anchor {
            StreamAnchor::Spoken {
                clause,
                caption_offset,
            } => {
                let clause_id = usize::try_from(clause)
                    .ok()
                    .and_then(|clause| stream.clauses.get(clause))
                    .copied();
                let offset = clause_id
                    .and_then(|clause| self.voice.clause(clause))
                    .map(|clause| clause.speakable.speakable_offset(caption_offset));
                match (clause_id, offset) {
                    (Some(clause), Some(offset)) => Anchor::Spoken { clause, offset },
                    (None, _) | (Some(_), None) => Anchor::Immediate { stream: index },
                }
            }
            StreamAnchor::Unanchored => Anchor::Immediate { stream: index },
        };
        #[cfg(feature = "sim")]
        if let Anchor::Spoken { clause, offset } = anchor {
            self.anchors.insert(id, (clause, offset));
        }
        self.queue.next_op = self.queue.next_op.max(id.0.saturating_add(1));
        self.queue.push(QueuedOp::new(id, op.op, anchor));
    }
}
