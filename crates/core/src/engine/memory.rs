//! Memory in the conversation (PRODUCT §7.4, LLD §2.8): what this session captured, the talker's
//! `remember` and `add_task` tools, and the controls the user says.
//!
//! The controls are cue tables (`conversation::cues`) the core answers itself, in three words or
//! fewer and without a talker round trip: "forget that" undoes the last capture, "don't remember
//! this session" leaves nothing in the vault (I10), and "yes, keep it" keeps a proposed memory.
//! The talker hears about them in a notice with its next request. Keeping a proposal follows the
//! change gate's rule (I9): only the user's next utterance can do it, never a tool result or repo
//! text, and words cut off by mute can only close it.
//!
//! The captures here mirror the store's: every `Remember` and `AcceptProposal` adds one, and
//! `Forget { Last }` removes the newest, so both sides always agree on what "that" is.

use crate::conversation::ConversationState;
use crate::conversation::cues::{self, MemoryCue};
use crate::effect::{Effect, ForgetScope};
use crate::engine::listen::ChangeGate;
use crate::engine::reply::Reply;
use crate::engine::text::{FORGOTTEN_LINE, KEPT_LINE, OFF_THE_RECORD_LINE};
use crate::engine::{Core, TurnMarks};
use crate::ids::CaptureId;
use crate::limits::{CAPTURES_PER_SESSION_MAX, LABEL_MAX_BYTES};
use crate::memory::{MemoryAction, MemoryKind, MemoryWrite, Subject};
use crate::memory_policy;
use crate::narration::speakable::Speakable;
use crate::narration::tags::bounded;
use crate::protocol::{ChipKind, UiEvent};
use crate::time::SessionTime;
use crate::tools::RememberKind;
use crate::voice::ClauseSource;

/// One thing the session captured, newest last.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Captured {
    text: String,
}

/// A proposed memory waiting for the user's next words.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Proposal {
    capture: CaptureId,
    text: String,
    /// When it was proposed: only words begun after it can keep it.
    opened_at: SessionTime,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Memory {
    captures: Vec<Captured>,
    next_capture: u32,
    /// The user said "don't remember this session": nothing more is written.
    off_record: bool,
    proposal: Option<Proposal>,
}

/// Why a capture was not sent to the store.
enum NotStored {
    OffRecord,
    Full,
    Never,
}

impl NotStored {
    fn tool_text(&self) -> String {
        match self {
            Self::OffRecord => {
                "Not stored: the user asked not to remember this session.".to_owned()
            }
            Self::Full => format!(
                "Not stored: this session already captured {CAPTURES_PER_SESSION_MAX} items."
            ),
            Self::Never => "Not stored: this kind of thing is never kept.".to_owned(),
        }
    }
}

impl Memory {
    fn take_id(&mut self) -> CaptureId {
        let id = CaptureId(self.next_capture);
        self.next_capture = self.next_capture.wrapping_add(1);
        id
    }

    /// Whether the user said "don't remember this session".
    #[must_use]
    pub(super) fn is_off_record(&self) -> bool {
        self.off_record
    }
}

impl Core {
    /// The talker's `remember`: the policy decides, the store keeps it, a chip flies.
    pub(super) fn remember(
        &mut self,
        now: SessionTime,
        kind: RememberKind,
        text: &str,
        about: Subject,
        links: &[String],
        effects: &mut Vec<Effect>,
    ) -> String {
        let kind = memory_policy::remember_kind(kind);
        let subject = match kind {
            MemoryKind::Note => Subject::You,
            MemoryKind::SessionRecord
            | MemoryKind::LearningSignal
            | MemoryKind::Stated
            | MemoryKind::Inferred
            | MemoryKind::Task
            | MemoryKind::RawAudio
            | MemoryKind::Secret => about,
        };
        let (capture, action) = match self.write_memory(kind, text, links, None, subject, effects) {
            Ok(written) => written,
            Err(reason) => return reason.tool_text(),
        };
        match action {
            MemoryAction::Keep => "Remembered.".to_owned(),
            MemoryAction::OnRequest => "Saved.".to_owned(),
            MemoryAction::Propose => self.open_proposal(now, capture, text),
            MemoryAction::Never => NotStored::Never.tool_text(),
        }
    }

    /// The talker's `add_task`.
    pub(super) fn add_task(
        &mut self,
        title: &str,
        project: Option<String>,
        links: &[String],
        effects: &mut Vec<Effect>,
    ) -> String {
        match self.write_memory(
            MemoryKind::Task,
            title,
            links,
            project,
            Subject::You,
            effects,
        ) {
            Ok(_) => "Task added.".to_owned(),
            Err(reason) => reason.tool_text(),
        }
    }

    /// The memory policy decides; anything not `Never` goes to the store with a chip on screen.
    fn write_memory(
        &mut self,
        kind: MemoryKind,
        text: &str,
        links: &[String],
        project: Option<String>,
        subject: Subject,
        effects: &mut Vec<Effect>,
    ) -> Result<(CaptureId, MemoryAction), NotStored> {
        let action = memory_policy::action(kind);
        if action == MemoryAction::Never {
            return Err(NotStored::Never);
        }
        if self.memory.off_record {
            return Err(NotStored::OffRecord);
        }
        if self.memory.captures.len() >= CAPTURES_PER_SESSION_MAX {
            return Err(NotStored::Full);
        }
        let capture = self.memory.take_id();
        effects.push(Effect::Remember(MemoryWrite {
            kind,
            action,
            text: text.to_owned(),
            links: links.to_vec(),
            project,
            subject,
            capture,
        }));
        self.memory.captures.push(Captured {
            text: bounded(text, LABEL_MAX_BYTES),
        });
        let chip = match kind {
            MemoryKind::Task => ChipKind::Task,
            MemoryKind::Note => ChipKind::Note,
            MemoryKind::SessionRecord
            | MemoryKind::LearningSignal
            | MemoryKind::Stated
            | MemoryKind::Inferred
            | MemoryKind::RawAudio
            | MemoryKind::Secret => ChipKind::Memory,
        };
        effects.push(Effect::Ui(UiEvent::Chip {
            kind: chip,
            text: bounded(text, LABEL_MAX_BYTES),
        }));
        Ok((capture, action))
    }

    /// An inferred memory waits for the user's next words, unless a code change already does:
    /// one yes can't answer two questions.
    fn open_proposal(&mut self, now: SessionTime, capture: CaptureId, text: &str) -> String {
        if matches!(self.gate, ChangeGate::Awaiting { .. }) {
            return "Proposed, not kept: it waits in the session digest. Don't ask about it now; \
                    a code change is waiting for the user's yes."
                .to_owned();
        }
        self.memory.proposal = Some(Proposal {
            capture,
            text: bounded(text, LABEL_MAX_BYTES),
            opened_at: now,
        });
        "Proposed, not kept. Ask in a few words whether to keep it (\"Want me to remember \
         that?\"). Only the user's next words can keep it; otherwise it waits in the session \
         digest."
            .to_owned()
    }

    /// A code change was proposed: its yes takes the floor, and an open proposal waits in the
    /// digest instead.
    pub(super) fn yield_proposal_to_change(&mut self) {
        if let Some(proposal) = self.memory.proposal.take() {
            self.context.notice(format!(
                "The proposed memory \"{}\" was not kept; it waits in the session digest. The \
                 user's next yes answers the code change.",
                proposal.text
            ));
        }
    }

    /// Mute cut the user's words off: they can close a proposal, never keep it.
    pub(super) fn close_proposal_on_mute(&mut self) {
        if let Some(proposal) = self.memory.proposal.take() {
            self.context.notice(format!(
                "The user muted the microphone mid-sentence, so the proposed memory \"{}\" was \
                 not kept; it waits in the session digest.",
                proposal.text
            ));
        }
    }

    /// The user's next words after a proposal: "yes, keep it" keeps it and is answered here
    /// (returns true); anything else leaves it in the digest and goes on to the talker. Words
    /// begun before the proposal (the user was talking over it) answer something else, and the
    /// proposal waits for the next ones.
    pub(super) fn resolve_proposal(
        &mut self,
        text: &str,
        began: SessionTime,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if self
            .memory
            .proposal
            .as_ref()
            .is_some_and(|proposal| proposal.opened_at > began)
        {
            return false;
        }
        let Some(proposal) = self.memory.proposal.take() else {
            return false;
        };
        if !cues::is_keep(text) {
            self.context.notice(format!(
                "The user did not keep the proposed memory \"{}\"; it waits in the session \
                 digest.",
                proposal.text
            ));
            return false;
        }
        effects.push(Effect::AcceptProposal {
            capture: proposal.capture,
        });
        self.memory.captures.push(Captured {
            text: proposal.text.clone(),
        });
        self.context.notice(format!(
            "The user said \"{text}\", so the proposed memory \"{}\" is kept. You said \
             \"{KEPT_LINE}\"",
            proposal.text
        ));
        self.say(KEPT_LINE, effects);
        true
    }

    /// "Forget that", "don't remember this session", or "close that" with a view open: the
    /// core answers these itself (returns true). "Forget that" with nothing captured is left to
    /// the talker: it usually means "never mind".
    pub(super) fn memory_control(&mut self, text: &str, effects: &mut Vec<Effect>) -> bool {
        match cues::memory_cue(text) {
            Some(MemoryCue::ForgetSession) => {
                if !self.memory.off_record {
                    self.memory.off_record = true;
                    self.memory.captures.clear();
                    self.memory.proposal = None;
                    effects.push(Effect::Forget {
                        scope: ForgetScope::Session,
                    });
                    self.context.notice(format!(
                        "The user asked not to remember this session. Nothing from it is kept, \
                         and remember and add_task store nothing from now on. You said \
                         \"{OFF_THE_RECORD_LINE}\""
                    ));
                }
                self.say(OFF_THE_RECORD_LINE, effects);
                true
            }
            Some(MemoryCue::ForgetLast) => {
                let Some(forgotten) = self.memory.captures.pop() else {
                    return false;
                };
                effects.push(Effect::Forget {
                    scope: ForgetScope::Last,
                });
                self.context.notice(format!(
                    "The user said \"{text}\", so the last thing captured (\"{}\") was removed \
                     from memory. You said \"{FORGOTTEN_LINE}\"",
                    forgotten.text
                ));
                self.say(FORGOTTEN_LINE, effects);
                true
            }
            None => self.close_view_by_voice(text, effects),
        }
    }

    /// A reply the core speaks itself: one short line, no talker request. The turn ends when
    /// it has played, like any reply.
    fn say(&mut self, line: &str, effects: &mut Vec<Effect>) {
        let clause = self.voice.add(
            line.to_owned(),
            Speakable::from_caption(line),
            ClauseSource::Core,
        );
        effects.push(Effect::Synthesize {
            clause,
            text: line.to_owned(),
        });
        self.reply = Some(Reply::spoken_by_core());
        self.marks = TurnMarks::default();
        self.state = ConversationState::Thinking;
    }
}

#[cfg(test)]
mod tests {
    use super::Captured;
    use crate::effect::Effect;
    use crate::engine::{Core, CoreConfig};
    use crate::limits::CAPTURES_PER_SESSION_MAX;
    use crate::memory::Subject;
    use crate::tools::RememberKind;

    /// The core stops at the store's bound itself, so the two never disagree on "that".
    #[test]
    fn captures_stop_at_the_session_bound() {
        let mut core = Core::new(CoreConfig::default());
        let full = Captured {
            text: "x".to_owned(),
        };
        core.memory.captures = vec![full; CAPTURES_PER_SESSION_MAX - 1];
        let mut effects = Vec::new();
        let kept = core.remember(
            crate::time::SessionTime::ZERO,
            RememberKind::Note,
            "last one",
            Subject::You,
            &[],
            &mut effects,
        );
        assert_eq!(kept, "Saved.");
        let refused = core.add_task("one too many", None, &[], &mut effects);
        assert!(refused.starts_with("Not stored"), "{refused}");
        let sent = effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Remember(_)))
            .count();
        assert_eq!(sent, 1);
    }
}
