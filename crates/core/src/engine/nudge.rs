//! One more request when a reply missed what its turn asked for (D-178, D-232).
//!
//! - **A repo read was written instead of called**: `read_file`, `list_dir` or `search_code`
//!   written as words (D-179) or as a tag (D-143), with no tool called in that stream. The repo
//!   tour then ends before it starts (gemma4: two of three tours spoke `list_dir path "."`).
//! - **A repo read was announced and not made** ("I will start by listing the root directory",
//!   `narration::spoken_calls` matches it), with no tool called in that stream, when the
//!   session's requests offer the repo reads over an open repo (`RepoReads::Offered`). gemma4's
//!   first turn did this in 16 to 18 of 20 repo tours (T-026).
//! - **A drawing was asked for** ("draw", "show me", "diagram": `draw_cues`) and the reply drew
//!   nothing and showed no card or view. The owner: "sometimes it keeps explaining without
//!   drawing even when I ask it to draw things".
//!
//! A read comes first: what a tour draws of the repo must come from what it read (D-180). The
//! core asks the talker once more within the same turn. What it said stays in the history as
//! it was, a short bracketed note follows it on the user's side (accepted by every provider,
//! unlike a trailing system message), and the next stream continues the spoken reply. At most
//! `NUDGES_PER_TURN_MAX` times a turn, only when a stream ended the turn normally, never while
//! a proposed change waits to be asked about or a stop settles, and never once the user has the
//! floor (drawing takes it: the reply stops, and no request goes out while they draw, I14).

use crate::conversation::ConversationState;
use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::gate::ChangeGate;
use crate::engine::reply::Stream;
use crate::engine::written::written_calls;
use crate::history::{Message, UserPart};
use crate::limits::NUDGES_PER_TURN_MAX;
use crate::narration::ops::NarrationOp;
use crate::time::SessionTime;
use crate::tools::ToolCall;

/// The repo reads a nudge asks to call for real.
const REPO_READS: &[&str] = &["read_file", "list_dir", "search_code"];

/// Whether the session's talker requests offer the repo reads over an open repo (D-232). The
/// runtime knows (the folders it opened, the tools it sends); only then is a read the speech
/// announced asked for as a call, since otherwise there is nothing it could have called.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoReads {
    /// Every request offers `read_file`, `list_dir` and `search_code`, and a folder is open.
    Offered,
    /// No folder is open, or the talker gets no tools.
    #[default]
    NotOffered,
}

/// Ops that show the user something: a stroke or a card beside the board.
pub(super) fn shows(op: &NarrationOp) -> bool {
    op.draws()
        || matches!(
            op,
            NarrationOp::Code { .. } | NarrationOp::Compare { .. } | NarrationOp::Recap { .. }
        )
}

/// Calls that show a view.
pub(super) fn is_view(call: &ToolCall) -> bool {
    match call {
        ToolCall::OpenSession { .. }
        | ToolCall::ShowLibrary
        | ToolCall::ShowTasks { .. }
        | ToolCall::ShowMemory
        | ToolCall::PlaySession { .. } => true,
        ToolCall::ReadFile { .. }
        | ToolCall::SearchCode { .. }
        | ToolCall::ListDir { .. }
        | ToolCall::Recall { .. }
        | ToolCall::Research { .. }
        | ToolCall::Review { .. }
        | ToolCall::ProposeChange { .. }
        | ToolCall::Remember { .. }
        | ToolCall::AddTask { .. } => false,
    }
}

/// The note that asks again.
pub const DRAW_NOTE: &str = "[A note from BoardAI, not the user's words: they asked for a \
    drawing and your reply drew nothing. Draw it on the board now while you explain it in a \
    few sentences; don't repeat what you already said.]";

/// The note for reads a stream wrote instead of calling.
fn written_note(reads: &[&str]) -> String {
    format!(
        "[A note from BoardAI, not the user's words: your reply wrote {} in its text, so \
         nothing ran, the user heard none of it, and you have not seen the repo: don't \
         describe it yet. Call it now as a tool call, not in your text, then go on.]",
        reads.join(" and ")
    )
}

/// The note for a read a stream announced and didn't make: `tool` is the one it implies.
pub fn announced_note(tool: &str) -> String {
    format!(
        "[A note from BoardAI, not the user's words: your reply said it would look at the repo \
         but called no tool, so nothing was read and you have not seen it: don't describe it \
         yet. Call {tool} now as a tool call, not in your text; don't repeat what you already \
         said.]"
    )
}

/// The repo reads a stream wrote as words or as tags instead of calling them, once each.
fn repo_reads_written(stream: &Stream) -> Vec<&'static str> {
    let spoken = stream.narration.spoken_calls().iter().map(|call| call.tool);
    let tagged = written_calls(stream.narration.written()).calls;
    let tagged = tagged.iter().filter_map(|call| {
        REPO_READS
            .iter()
            .copied()
            .find(|read| *read == call.name.as_str())
    });
    let mut reads = Vec::new();
    for tool in spoken.chain(tagged) {
        if REPO_READS.contains(&tool) && !reads.contains(&tool) {
            reads.push(tool);
        }
    }
    reads
}

impl Core {
    /// The first repo read stream `index` announced and didn't make, when one could have been
    /// made (D-232).
    fn announced_read(&self, index: usize) -> Option<&'static str> {
        let stream = self.reply.as_ref()?.streams.get(index)?;
        match self.config.repo_reads {
            RepoReads::Offered if !stream.has_calls() => {
                stream.narration.announced_reads().first().copied()
            }
            RepoReads::Offered | RepoReads::NotOffered => None,
        }
    }

    /// A stream that wrote or announced a repo read instead of calling it was touring the repo
    /// all the same: what it draws of the repo is checked against what was read (D-180).
    pub(super) fn mark_tour(&mut self, index: usize) {
        let wrote = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
            .is_some_and(|stream| !repo_reads_written(stream).is_empty());
        let announced = self.announced_read(index).is_some();
        if let Some(reply) = self.reply.as_mut() {
            reply.touring |= wrote || announced;
        }
    }

    /// Stream `index` ended the turn: ask the talker once more when it missed what the turn
    /// asked for. Returns true when it did.
    pub(super) fn maybe_nudge(
        &mut self,
        now: SessionTime,
        index: usize,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let floor_is_ours = matches!(
            self.state,
            ConversationState::Thinking | ConversationState::Speaking
        );
        let change_waits = matches!(self.gate, ChangeGate::Proposed { .. });
        let Some(reply) = self
            .reply
            .as_ref()
            .filter(|reply| reply.nudges < NUDGES_PER_TURN_MAX)
            .filter(|_| floor_is_ours && !change_waits && self.stop.is_none())
        else {
            return false;
        };
        let Some(stream) = reply.streams.get(index) else {
            return false;
        };
        let reads = if stream.has_calls() {
            Vec::new()
        } else {
            repo_reads_written(stream)
        };
        let note = if !reads.is_empty() {
            written_note(&reads)
        } else if let Some(tool) = self.announced_read(index) {
            announced_note(tool)
        } else if reply.asks_drawing && !reply.shown {
            DRAW_NOTE.to_owned()
        } else {
            return false;
        };
        if let Some(reply) = self.reply.as_mut() {
            reply.nudges += 1;
            reply.done = false;
        }
        self.flush_user();
        self.conversation.push(Message::User {
            parts: vec![UserPart::Text { text: note }],
        });
        self.request_talk(now, effects);
        true
    }
}
