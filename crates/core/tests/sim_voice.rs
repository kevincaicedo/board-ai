//! Simulator scenarios: the voice failing under a reply (LLD §2.1, §6). A clause whose synthesis
//! failed must never hold the turn open, and nothing anchored in it may be drawn (I1).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::Effect;
use boardai_core::ids::ClauseId;
use boardai_core::protocol::{DrawOp, OrbState, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{DEMO, assert_clean, config, op_starts, orbs, transcript};

fn drawn_ids(sim: &Sim) -> Vec<String> {
    op_starts(sim)
        .iter()
        .filter_map(|(_, event)| {
            if let UiEvent::OpStart {
                op: DrawOp::Node { id, .. },
                ..
            } = event
            {
                Some(id.as_str().to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn problems(sim: &Sim) -> Vec<String> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::Problem { spoken, .. } = event {
                Some(spoken.clone())
            } else {
                None
            }
        })
        .collect()
}

/// The demo's last clause ("Then the load balancer spreads it across three API pods.") fails to
/// synthesize. The load balancer and the pods are never drawn, the user reads that clause
/// instead, and the reply still ends and goes idle.
#[test]
fn a_clause_that_fails_to_synthesize_is_skipped_and_the_reply_ends() {
    let mut sim = Sim::new(config());
    sim.speech.fail(ClauseId(2));
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    assert_clean(&sim);
    let drawn = drawn_ids(&sim);
    assert_eq!(drawn, vec!["client".to_owned(), "cdn".to_owned()]);
    assert_eq!(
        problems(&sim),
        vec!["Then the load balancer spreads it across three API pods.".to_owned()]
    );
    assert_eq!(*sim.core().state(), ConversationState::Idle);
    assert_eq!(orbs(&sim).last(), Some(&OrbState::Idle));
    let history = transcript(&sim);
    assert!(
        history[2].starts_with("assistant(stored): <plan"),
        "{history:#?}"
    );
}

/// No clause can be said at all: nothing is drawn and the turn still ends, so the user can ask
/// again.
#[test]
fn a_reply_whose_every_clause_fails_still_ends() {
    let mut sim = Sim::new(config());
    for clause in 0..8 {
        sim.speech.fail(ClauseId(clause));
    }
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says("Sure, again."));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(20_000);
    assert_clean(&sim);
    assert!(op_starts(&sim).is_empty());
    assert_eq!(problems(&sim).len(), 3, "one per clause of the demo");
    assert_eq!(*sim.core().state(), ConversationState::Idle);
    sim.user_says(20_000, "Can you try again?");
    sim.run_until(40_000);
    let talks = sim
        .effects()
        .filter(|(_, effect)| matches!(effect, Effect::Talk { .. }))
        .count();
    assert_eq!(talks, 2, "the next question is answered");
}
