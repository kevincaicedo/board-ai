//! What speech in should do, as the core's state says (LLD §3.1). The core decides when it
//! listens (mute, push-to-talk); the runtime only carries that out, so the orchestrator
//! publishes it after every step on a `watch` and any speech-in source follows it:
//!
//! - **open**: whether the mic is heard. Closed while muted; in push-to-talk mode, open only
//!   while the user holds the floor (from the key press until the turn ends), so speech without
//!   the key is never even transcribed.
//! - **recent terms**: labels BoardAI drew, newest first, for speech-to-text to expect ("Redis",
//!   "Load balancer"), since the user tends to say them back.

use std::collections::VecDeque;
use std::sync::Arc;

use boardai_core::MicMode;
use boardai_core::conversation::ConversationState;
use boardai_core::protocol::{DrawOp, UiEvent};
use tokio::sync::watch;

use crate::limits::{VOCABULARY_RECENT_TERMS_MAX, VOCABULARY_TERM_MAX_BYTES};

/// What speech in should do now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listening {
    pub open: bool,
    /// Newest first, at most [`VOCABULARY_RECENT_TERMS_MAX`], no two equal ignoring case.
    pub recent_terms: VecDeque<String>,
}

/// Whether the core would hear the mic in `state`.
#[must_use]
pub fn mic_open(mode: MicMode, state: &ConversationState) -> bool {
    match (mode, state) {
        (MicMode::OpenMic | MicMode::PushToTalk, ConversationState::Muted) => false,
        (MicMode::OpenMic, _) => true,
        // Push-to-talk reaches Listening only through the key, and keeps it until the turn ends
        // (the final transcript of a released key still has to arrive).
        (MicMode::PushToTalk, ConversationState::Listening(_) | ConversationState::Ducked(_)) => {
            true
        }
        (
            MicMode::PushToTalk,
            ConversationState::Idle | ConversationState::Thinking | ConversationState::Speaking,
        ) => false,
    }
}

/// The orchestrator's side: it publishes after every step.
#[derive(Debug)]
pub(crate) struct ListeningPublisher {
    mode: MicMode,
    sender: Arc<watch::Sender<Listening>>,
}

impl ListeningPublisher {
    pub(crate) fn new(mode: MicMode, sender: Arc<watch::Sender<Listening>>) -> Self {
        Self { mode, sender }
    }

    /// Publishes the mic state for `state` and the terms drawn in `events`; receivers wake only
    /// when something changed.
    pub(crate) fn after_step(&self, state: &ConversationState, events: &[UiEvent]) {
        let open = mic_open(self.mode, state);
        self.sender.send_if_modified(|listening| {
            let mut changed = listening.open != open;
            listening.open = open;
            for term in events.iter().filter_map(drawn_term) {
                changed |= remember(&mut listening.recent_terms, term);
            }
            changed
        });
    }
}

/// A label the board just drew: a node's, a participant's or a row's, or an edge's or a
/// message's when it has one.
fn drawn_term(event: &UiEvent) -> Option<&str> {
    let UiEvent::OpStart { op, .. } = event else {
        return None;
    };
    match op {
        DrawOp::Node { label, .. }
        | DrawOp::Participant { label, .. }
        | DrawOp::Cells { label, .. } => Some(label),
        DrawOp::Edge { label, .. } | DrawOp::Message { label, .. } => label.as_deref(),
        DrawOp::Annotate { .. }
        | DrawOp::Strike { .. }
        | DrawOp::Trace { .. }
        | DrawOp::Write { .. } => None,
    }
}

/// Puts `term` first; true when the list changed. Empty and over-long labels are not terms.
fn remember(terms: &mut VecDeque<String>, term: &str) -> bool {
    let term = term.trim();
    if term.is_empty() || term.len() > VOCABULARY_TERM_MAX_BYTES {
        return false;
    }
    if terms
        .front()
        .is_some_and(|first| first.eq_ignore_ascii_case(term))
    {
        return false;
    }
    terms.retain(|known| !known.eq_ignore_ascii_case(term));
    terms.push_front(term.to_owned());
    terms.truncate(VOCABULARY_RECENT_TERMS_MAX);
    true
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use boardai_core::MicMode;
    use boardai_core::conversation::ConversationState;
    use boardai_core::conversation::utterance::Utterance;
    use boardai_core::geometry::Rect;
    use boardai_core::ids::{ElementId, LayerId, OpId};
    use boardai_core::protocol::{DrawOp, NodeShape, UiEvent};
    use boardai_core::time::SessionTime;
    use tokio::sync::watch;

    use super::{Listening, ListeningPublisher, mic_open, remember};
    use crate::limits::VOCABULARY_RECENT_TERMS_MAX;

    fn every_state() -> [ConversationState; 5] {
        [
            ConversationState::Idle,
            ConversationState::Listening(Utterance::start(SessionTime::ZERO, true)),
            ConversationState::Thinking,
            ConversationState::Speaking,
            ConversationState::Muted,
        ]
    }

    #[test]
    fn open_mic_hears_everything_but_mute() {
        for state in every_state() {
            let expected = state != ConversationState::Muted;
            assert_eq!(
                mic_open(MicMode::OpenMic, &state),
                expected,
                "{}",
                state.name()
            );
        }
    }

    #[test]
    fn push_to_talk_hears_only_while_the_user_has_the_floor() {
        for state in every_state() {
            let expected = matches!(state, ConversationState::Listening(_));
            assert_eq!(
                mic_open(MicMode::PushToTalk, &state),
                expected,
                "{}",
                state.name()
            );
        }
    }

    fn node(label: &str) -> UiEvent {
        UiEvent::OpStart {
            layer: LayerId::MAIN,
            op: DrawOp::Node {
                id: ElementId::parse("n").expect("a valid id"),
                op: OpId(0),
                shape: NodeShape::Service,
                label: label.to_owned(),
                rect: Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                },
                stack: 1,
                fill: None,
                link: None,
            },
            budget_ms: 300,
        }
    }

    #[test]
    fn drawn_labels_become_recent_terms_newest_first() {
        let sender = Arc::new(watch::Sender::new(Listening::default()));
        let mut receiver = sender.subscribe();
        let publisher = ListeningPublisher::new(MicMode::OpenMic, Arc::clone(&sender));
        publisher.after_step(&ConversationState::Speaking, &[node("CDN"), node("Redis")]);
        assert!(receiver.has_changed().unwrap_or(false));
        let seen = receiver.borrow_and_update().clone();
        assert!(seen.open);
        assert_eq!(
            seen.recent_terms,
            VecDeque::from(["Redis".to_owned(), "CDN".to_owned()])
        );
        // The same state and a label already first: nothing to wake anyone for.
        publisher.after_step(&ConversationState::Speaking, &[node("redis")]);
        assert!(!receiver.has_changed().unwrap_or(true));
        publisher.after_step(&ConversationState::Muted, &[]);
        assert!(!receiver.borrow_and_update().open);
    }

    #[test]
    fn terms_are_bounded_and_unique() {
        let mut terms = VecDeque::new();
        for index in 0..VOCABULARY_RECENT_TERMS_MAX + 5 {
            assert!(remember(&mut terms, &format!("term {index}")));
        }
        assert_eq!(terms.len(), VOCABULARY_RECENT_TERMS_MAX);
        assert_eq!(terms.front().map(String::as_str), Some("term 36"));
        assert!(remember(&mut terms, "TERM 20"));
        assert_eq!(
            terms
                .iter()
                .filter(|t| t.eq_ignore_ascii_case("term 20"))
                .count(),
            1
        );
        assert!(!remember(&mut terms, "   "));
        assert!(!remember(&mut terms, &"x".repeat(200)));
    }
}
