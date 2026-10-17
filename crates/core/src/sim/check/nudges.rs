//! Law 5 for asking the talker once more (D-178, D-232, D-249): a turn asks again at most
//! `NUDGES_PER_TURN_MAX + READ_DRAW_NUDGES_PER_TURN_MAX` times, so a model that keeps missing
//! never holds the floor in a loop. A request is a nudge when the last message before its context
//! is BoardAI's bracketed note; a tool round's results continue the turn, and anything else (the
//! user's words, a harness report) starts the next one.

use crate::effect::TalkRequest;
use crate::engine::NUDGE_NOTE_PREFIX;
use crate::history::{Message, UserPart};
use crate::limits::{NUDGES_PER_TURN_MAX, READ_DRAW_NUDGES_PER_TURN_MAX};
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct Nudges {
    /// Requests of the current turn that asked again.
    this_turn: usize,
}

/// What a request continues.
enum Asks {
    Again,
    ToolRound,
    NewTurn,
}

impl Nudges {
    /// `Talk`: counts the turn's nudges and flags one past the bound.
    pub fn on_talk(
        &mut self,
        now: SessionTime,
        request: &TalkRequest,
        violations: &mut Vec<String>,
    ) {
        match asks(request) {
            Asks::Again => self.this_turn += 1,
            Asks::ToolRound => {}
            Asks::NewTurn => self.this_turn = 0,
        }
        let bound = NUDGES_PER_TURN_MAX + READ_DRAW_NUDGES_PER_TURN_MAX;
        if self.this_turn > bound {
            violations.push(format!(
                "{} ms: a turn asked the talker again {} times; the bound is {bound}",
                now.as_ms(),
                self.this_turn
            ));
        }
    }
}

fn asks(request: &TalkRequest) -> Asks {
    let last = request
        .messages
        .iter()
        .rev()
        .find(|message| !matches!(message, Message::System { .. }));
    let Some(Message::User { parts }) = last else {
        return Asks::NewTurn;
    };
    if parts
        .iter()
        .any(|part| matches!(part, UserPart::ToolResult { .. }))
    {
        return Asks::ToolRound;
    }
    let noted = parts.iter().any(|part| match part {
        UserPart::Text { text } => text.starts_with(NUDGE_NOTE_PREFIX),
        UserPart::ToolResult { .. } => false,
    });
    if noted { Asks::Again } else { Asks::NewTurn }
}

#[cfg(test)]
mod tests {
    use super::Nudges;
    use crate::effect::TalkRequest;
    use crate::engine::NUDGE_NOTE_PREFIX;
    use crate::history::{Message, UserPart};
    use crate::limits::{NUDGES_PER_TURN_MAX, READ_DRAW_NUDGES_PER_TURN_MAX};
    use crate::time::SessionTime;

    fn user(text: &str) -> Message {
        Message::User {
            parts: vec![UserPart::Text {
                text: text.to_owned(),
            }],
        }
    }

    fn talk(nudges: &mut Nudges, history: &mut Vec<Message>, last: Message) -> Vec<String> {
        history.push(last);
        history.push(Message::System {
            text: "Board now: empty".to_owned(),
        });
        let mut violations = Vec::new();
        let request = TalkRequest {
            messages: history.clone(),
        };
        nudges.on_talk(SessionTime::from_ms(1), &request, &mut violations);
        violations
    }

    /// The planted violation: one note past the bound in a turn turns the check red; the same
    /// count spread over two turns doesn't.
    #[test]
    fn a_turn_asking_past_the_bound_is_caught() {
        let note = user(&format!("{NUDGE_NOTE_PREFIX} draw it now.]"));
        let bound = NUDGES_PER_TURN_MAX + READ_DRAW_NUDGES_PER_TURN_MAX;
        let mut nudges = Nudges::default();
        let mut history = Vec::new();
        assert!(talk(&mut nudges, &mut history, user("Walk me through this repo")).is_empty());
        for _ in 0..bound {
            assert!(talk(&mut nudges, &mut history, note.clone()).is_empty());
        }
        let past = talk(&mut nudges, &mut history, note.clone());
        assert_eq!(past.len(), 1, "{past:?}");
        assert!(talk(&mut nudges, &mut history, user("And the crates?")).is_empty());
        for _ in 0..bound {
            assert!(talk(&mut nudges, &mut history, note.clone()).is_empty());
        }
    }
}
