//! The change confirmation gate (LLD §3.5, I9): a code change runs only on the user's spoken yes
//! to a question BoardAI asked itself, naming the branch and the exact instruction that runs.
//!
//! - `propose_change` is checked and held, never armed: one proposal at a time, and only an
//!   instruction short enough to read out word for word.
//! - When the talker's reply ends, the core asks: it speaks, and so captions, a fixed line built
//!   from the instruction and branch (`text::change_question`).
//! - The gate opens when that line's audio has played. The next utterance resolves it: a yes
//!   begun after the gate opened runs exactly that instruction; anything else closes it.
//! - A proposal whose question never played to the end is dropped: the reply was cut off
//!   (an interruption, Esc, mute, a new question while thinking), it failed or was refused, or the
//!   voice could not say the line. Esc, mute or a harness report turn close an open gate too.
//!
//! Tool results and repo text never reach any of this: only the user's words resolve the gate.

use crate::conversation::cues;
use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::text::{branch_for, change_question};
use crate::harness::{HarnessKind, HarnessTask};
use crate::history::UserPart;
use crate::ids::{ClauseId, ToolCallId};
use crate::limits::CHANGE_INSTRUCTION_MAX_BYTES;
use crate::narration::speakable::Speakable;
use crate::time::SessionTime;
use crate::voice::ClauseSource;

/// A change as the user will hear it: the instruction, whitespace collapsed, and its branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Change {
    instruction: String,
    branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ChangeGate {
    Closed,
    /// Proposed by stream `stream` of the reply in progress; asked when the reply ends.
    Proposed {
        change: Change,
        stream: usize,
    },
    /// BoardAI's question is queued or playing as `clause`.
    Asking {
        change: Change,
        stream: usize,
        clause: ClauseId,
    },
    /// The question finished playing at `opened_at`; the user's next utterance answers it.
    Open {
        change: Change,
        opened_at: SessionTime,
    },
}

impl ChangeGate {
    pub(super) fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Why an open gate closed without running anything; the talker is told which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GateClosed {
    /// The user's next words were not a yes begun after the question.
    NotYes,
    /// Esc.
    Stop,
    Mute,
    /// A harness report turn started before the user answered.
    Report,
    /// The user started drawing instead of answering (artboard 13).
    Drawing,
}

impl GateClosed {
    fn notice(self) -> &'static str {
        match self {
            Self::NotYes => {
                "The user's next words after BoardAI's question were not a yes, so the proposed \
                 change was cancelled. Nothing was changed."
            }
            Self::Stop => {
                "The user pressed Stop, so the proposed change was cancelled. Nothing was changed."
            }
            Self::Mute => {
                "The user muted the microphone, so the proposed change was cancelled. Nothing was \
                 changed."
            }
            Self::Report => {
                "A background task reported before the user answered BoardAI's question, so the \
                 proposed change was cancelled. Nothing was changed; propose it again if it still \
                 makes sense."
            }
            Self::Drawing => {
                "The user started drawing instead of answering, so the proposed change was \
                 cancelled. Nothing was changed."
            }
        }
    }
}

impl Core {
    /// `propose_change`, from stream `stream` of the reply in progress: held until the reply
    /// ends, or refused with an error result.
    pub(super) fn propose_change(
        &mut self,
        id: &ToolCallId,
        instruction: &str,
        stream: usize,
    ) -> UserPart {
        let refused = |content: String| UserPart::ToolResult {
            id: id.clone(),
            content,
            is_error: true,
        };
        if !self.gate.is_closed() {
            return refused(
                "Error: a change is already waiting for the user's answer, and only one can be \
                 proposed at a time. This one was not proposed."
                    .to_owned(),
            );
        }
        let instruction = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
        if instruction.is_empty() {
            return refused("Error: the instruction is empty.".to_owned());
        }
        if instruction.len() > CHANGE_INSTRUCTION_MAX_BYTES {
            return refused(format!(
                "Error: the instruction is too long to read out ({} bytes, at most \
                 {CHANGE_INSTRUCTION_MAX_BYTES}). BoardAI reads it to the user word for word and \
                 runs exactly that, so propose it again as one short sentence.",
                instruction.len()
            ));
        }
        let branch = branch_for(&instruction);
        let question = change_question(&branch, &instruction);
        self.gate = ChangeGate::Proposed {
            change: Change {
                instruction,
                branch,
            },
            stream,
        };
        self.yield_proposal_to_change();
        UserPart::ToolResult {
            id: id.clone(),
            content: format!(
                "Nothing has changed yet. When your reply ends, BoardAI asks the user itself, word \
                 for word: \"{question}\" Don't ask for the yes yourself or describe the change \
                 again; finish what you were saying in a sentence at most. The change runs only \
                 if the user's next words after that question are a yes, and you will be told \
                 either way."
            ),
            is_error: false,
        }
    }

    /// The reply ended normally: BoardAI asks about the proposed change, as the reply's last
    /// clause.
    pub(super) fn ask_change(&mut self, effects: &mut Vec<Effect>) {
        let ChangeGate::Proposed { change, stream } = &self.gate else {
            return;
        };
        let question = change_question(&change.branch, &change.instruction);
        let speakable = Speakable::from_caption(&question);
        let text = speakable.text().to_owned();
        let clause = self.voice.add(question, speakable, ClauseSource::Core);
        effects.push(Effect::Synthesize { clause, text });
        self.gate = ChangeGate::Asking {
            change: change.clone(),
            stream: *stream,
            clause,
        };
    }

    /// Opens the gate once the question has played to its end. A question cut by a stop that is
    /// still settling never opens it, and one the voice could not say drops the proposal.
    pub(super) fn arm_change_gate(&mut self, now: SessionTime) {
        let ChangeGate::Asking { change, clause, .. } = &self.gate else {
            return;
        };
        if self.stop.is_some() {
            return;
        }
        if self.voice.played(*clause) {
            self.gate = ChangeGate::Open {
                change: change.clone(),
                opened_at: now,
            };
        } else if self.voice.skipped(*clause) {
            self.gate = ChangeGate::Closed;
            self.context.notice(
                "BoardAI's voice could not say its question about the proposed change, so the \
                 change was dropped. Nothing was changed."
                    .to_owned(),
            );
        }
    }

    /// The reply is over and its question never played to the end: the proposal is dropped. The
    /// talker hears why only if its call is in the history (its stream was committed).
    pub(super) fn drop_unasked_change(&mut self) {
        let stream = match &self.gate {
            ChangeGate::Proposed { stream, .. } | ChangeGate::Asking { stream, .. } => *stream,
            ChangeGate::Closed | ChangeGate::Open { .. } => return,
        };
        self.gate = ChangeGate::Closed;
        let told = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(stream))
            .is_some_and(|stream| stream.committed);
        if told {
            self.context.notice(
                "The proposed change was dropped: the user never heard BoardAI's whole question \
                 about it. Nothing was changed; propose it again if it still makes sense."
                    .to_owned(),
            );
        }
    }

    /// Closes a gate waiting for the user's answer.
    pub(super) fn close_change_gate(&mut self, why: GateClosed) {
        if let ChangeGate::Open { .. } = self.gate {
            self.gate = ChangeGate::Closed;
            self.context.notice(why.notice().to_owned());
        }
    }

    /// The user's utterance after the question (I9): a plain yes begun after the gate opened runs
    /// the change; anything else closes the gate.
    pub(super) fn resolve_gate(
        &mut self,
        text: &str,
        began: SessionTime,
        effects: &mut Vec<Effect>,
    ) {
        let ChangeGate::Open { change, opened_at } = &self.gate else {
            return;
        };
        if began < *opened_at || !cues::is_affirmative(text) {
            self.close_change_gate(GateClosed::NotYes);
            return;
        }
        let Change {
            instruction,
            branch,
        } = change.clone();
        self.gate = ChangeGate::Closed;
        let kind = HarnessKind::Change {
            branch: branch.clone(),
        };
        let task = self.start_task(kind.clone());
        effects.push(Effect::RunHarness {
            task,
            spec: HarnessTask {
                kind,
                brief: instruction,
            },
        });
        self.context.notice(format!(
            "The user said yes to BoardAI's question. Change task {} is running on branch \
             {branch}; its output arrives later, fenced as untrusted data.",
            task.0
        ));
    }
}
