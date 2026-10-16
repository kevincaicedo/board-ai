//! Simulator scenarios for "open settings" (LLD §3.8): the core opens the settings screen
//! itself, with a nod and no talker round trip, and tells the talker afterwards. Words that
//! only mention settings stay a question for the talker.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::Effect;
use boardai_core::history::Message;
use boardai_core::protocol::UiEvent;
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use common::{DEMO, assert_clean, config, count};

fn opened(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::OpenSettings))
    })
}

fn talks(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Talk { .. }))
}

fn spoken(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Synthesize { .. }))
}

#[test]
fn open_settings_opens_the_screen_with_a_nod_and_no_talker() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "open settings");
    sim.run_until(4_000);
    assert_clean(&sim);
    assert_eq!(opened(&sim), 1);
    assert_eq!(talks(&sim), 0, "no talker round trip");
    assert_eq!(spoken(&sim), 0, "a nod, no words");
    let events: Vec<&UiEvent> = sim.ui().map(|(_, event)| event).collect();
    let open = events
        .iter()
        .position(|event| matches!(event, UiEvent::OpenSettings))
        .expect("opened");
    assert!(matches!(events.get(open + 1), Some(UiEvent::Acknowledge)));

    sim.user_says(4_000, "okay, please show me the settings");
    sim.run_until(8_000);
    assert_eq!(opened(&sim), 2, "fillers around the phrase are fine");
    assert_eq!(talks(&sim), 0);

    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(8_000, "what's a good default model");
    sim.run_until(14_000);
    assert_clean(&sim);
    let request = sim.talker.requests.last().expect("a request");
    let told = request.messages.iter().any(|message| {
        matches!(message, Message::System { text } if text.contains("settings screen opened"))
    });
    assert!(told, "the talker hears that the settings screen opened");
}

#[test]
fn words_about_settings_are_a_question_for_the_talker() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("It reads the TOML."));
    sim.talker.push(ScriptedReply::says("Here it is."));
    sim.user_says(0, "open the settings file in this repo");
    sim.run_until(6_000);
    sim.user_says(6_000, "how does it load settings");
    sim.run_until(12_000);
    assert_clean(&sim);
    assert_eq!(opened(&sim), 0);
    assert_eq!(talks(&sim), 2);
}

#[test]
fn open_settings_over_an_explanation_interrupts_it_and_opens_the_screen() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "how does a request reach the API");
    sim.run_until(4_000);
    let before = talks(&sim);
    sim.user_says(4_000, "open settings");
    sim.run_until(12_000);
    assert_clean(&sim);
    assert_eq!(opened(&sim), 1);
    assert_eq!(
        talks(&sim),
        before,
        "the explanation is not asked for again"
    );
}
