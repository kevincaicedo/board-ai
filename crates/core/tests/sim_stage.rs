//! Simulator scenarios for the stage itself: ending the session by voice (D-144), going back to
//! the main board from a detour (D-147), and the recap leaving when the topic moves on (D-148).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::Effect;
use boardai_core::engine::text::{SESSION_ENDED_LINE, SESSION_SAVED_LINE};
use boardai_core::history::Message;
use boardai_core::ids::LayerId;
use boardai_core::input::Input;
use boardai_core::protocol::{Surface, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::views::{View, ViewKind};
use common::{DEMO, assert_clean, config, count};

fn ends(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::SessionEnd))
    })
}

fn talks(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Talk { .. }))
}

fn said(sim: &Sim, line: &str) -> bool {
    sim.effects()
        .any(|(_, effect)| matches!(effect, Effect::Synthesize { text, .. } if text == line))
}

/// What the last talker request told it in system messages.
fn last_notices(sim: &Sim) -> String {
    sim.talker
        .requests
        .last()
        .map(|request| {
            request
                .messages
                .iter()
                .filter_map(|message| match message {
                    Message::System { text } => Some(text.as_str()),
                    Message::User { .. } | Message::Assistant { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The owner's words for finishing (session log of 2026-10-16) end the session: the core says so
/// in two words without the talker, and `SessionEnd` comes once that has played.
#[test]
fn we_can_stop_here_ends_the_session_once_its_line_played() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    let before = talks(&sim);
    sim.user_says(30_000, "I think that is okay, I think we can stop here.");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), before, "no talker round trip");
    assert!(said(&sim, SESSION_SAVED_LINE));
    assert_eq!(ends(&sim), 1);
    let line_at = sim
        .effects()
        .find(|(_, effect)| matches!(effect, Effect::Synthesize { text, .. } if text == SESSION_SAVED_LINE))
        .map(|(at, _)| at)
        .expect("said");
    let end_at = sim
        .effects()
        .find(|(_, effect)| matches!(effect, Effect::Ui(UiEvent::SessionEnd)))
        .map(|(at, _)| at)
        .expect("ended");
    assert!(end_at > line_at, "the line plays first");
}

/// Talking over "Session saved." means the user is still here: nothing ends.
#[test]
fn an_end_line_cut_off_ends_nothing() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Sure, Raft elects a leader first."));
    sim.user_says(0, "end the session");
    assert!(sim.run_until_true(10_000, |sim| sim.speech.word_starts().len() > 1));
    let at = sim.now().as_ms();
    sim.user_says_with_onset(at, "wait, actually, explain Raft");
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    assert_eq!(ends(&sim), 0, "the session goes on");
    assert_eq!(talks(&sim), 1, "the new words go to the talker");
}

#[test]
fn an_off_the_record_session_ends_without_saving() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "don't remember this session");
    sim.run_until(5_000);
    sim.user_says(5_000, "Can you end the session?");
    sim.run_until(12_000);
    assert_clean(&sim);
    assert!(said(&sim, SESSION_ENDED_LINE));
    assert!(!said(&sim, SESSION_SAVED_LINE));
    assert_eq!(ends(&sim), 1);
}

#[test]
fn words_about_sessions_stay_a_question_for_the_talker() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("It ends when you say so."));
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(0, "how does a session end?");
    sim.run_until(8_000);
    sim.user_says(8_000, "explain Paxos briefly, we can stop here");
    sim.run_until(16_000);
    assert_clean(&sim);
    assert_eq!(ends(&sim), 0);
    assert_eq!(talks(&sim), 2);
}

/// A detour whose answer never wrote `<resume/>`, as in the owner's session.
const DETOUR_NO_RESUME: &str = concat!(
    r#"<detour title="cache-aside"/>That's the cache-aside pattern. "#,
    r#"<node id="redis" label="Redis" kind="db"/>Reads go to Redis first."#,
);

/// Asks the demo, interrupts it with a question, and lets the detour's answer (which never
/// resumes) finish.
fn detour_left_open(sim: &mut Sim) -> u64 {
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(DETOUR_NO_RESUME));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| sim.speech.word_starts().len() > 8));
    let at = sim.now().as_ms();
    sim.user_says_with_onset(at, "wait, why Redis first?");
    sim.run_until(at + 30_000);
    assert_eq!(sim.core().thread().depth(), 1, "the detour is still open");
    at + 30_000
}

fn pops(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::LayerPop { .. }))
    })
}

#[test]
fn back_to_the_main_board_closes_every_detour_and_the_talker_bridges() {
    let mut sim = Sim::new(config());
    let at = detour_left_open(&mut sim);
    // The talker resumes too: with no detour open, that changes nothing.
    sim.talker.push(ScriptedReply::says(
        r#"<resume/>So, back to the request. <node id="pg" label="Postgres" kind="db"/>Then Postgres."#,
    ));
    sim.user_says(at, "Okay, got it, back to the main board please.");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(pops(&sim), 1, "one detour, one pop");
    assert_eq!(sim.core().thread().depth(), 0);
    assert!(last_notices(&sim).contains("every open detour closed (\"cache-aside\")"));
    let drawn_on_main = sim.ui().any(|(_, event)| {
        matches!(event, UiEvent::OpStart { layer, op: boardai_core::protocol::DrawOp::Node { id, .. }, .. }
            if *layer == LayerId::MAIN && id.as_str() == "pg")
    });
    assert!(drawn_on_main, "the bridge draws on the main board");
}

/// "Go on" once the detour's answer has ended: the core resumes it if the talker didn't.
#[test]
fn go_on_after_a_detour_answer_continues_on_the_main_board() {
    let mut sim = Sim::new(config());
    let at = detour_left_open(&mut sim);
    sim.talker.push(ScriptedReply::says(
        r#"So, back to the request. <node id="pg" label="Postgres" kind="db"/>Then Postgres."#,
    ));
    sim.user_says(at, "Ok, that makes sense, go on.");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(pops(&sim), 1);
    assert_eq!(sim.core().thread().depth(), 0);
}

/// "Go on" said over the detour's answer asks for more of it: the detour stays open.
#[test]
fn go_on_over_a_detour_answer_keeps_the_detour() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="cache-aside"/>That's the cache-aside pattern, and it is worth a "#,
        r#"long look, because every read goes to Redis first and only then to Postgres."#,
    )));
    sim.talker.push(ScriptedReply::says(
        "Right, and on a miss it reads Postgres.",
    ));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| sim.speech.word_starts().len() > 8));
    let at = sim.now().as_ms();
    sim.user_says_with_onset(at, "wait, why Redis first?");
    assert!(sim.run_until_true(at + 30_000, |sim| sim.core().thread().depth() == 1));
    assert!(sim.run_until_true(at + 30_000, |sim| sim.speech.is_active()));
    let over = sim.now().as_ms() + 600;
    sim.user_says_with_onset(over, "go on, keep going");
    sim.run_until(over + 30_000);
    assert_clean(&sim);
    assert_eq!(pops(&sim), 0);
    assert_eq!(sim.core().thread().depth(), 1);
}

#[test]
fn go_back_with_no_detour_open_is_a_question_for_the_talker() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Back to what?"));
    sim.user_says(0, "go back");
    sim.run_until(8_000);
    assert_clean(&sim);
    assert_eq!(pops(&sim), 0);
    assert_eq!(talks(&sim), 1);
}

fn recap_ids(sim: &Sim) -> Vec<u32> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::SurfaceShow {
                surface: Surface::Recap { id, .. },
            } = event
            {
                Some(id.0)
            } else {
                None
            }
        })
        .collect()
}

fn dismissed(sim: &Sim, id: u32) -> bool {
    sim.ui()
        .any(|(_, event)| matches!(event, UiEvent::SurfaceDismiss { id: dismissed } if dismissed.0 == id))
}

/// PRODUCT §7.2: surfaces leave when the topic moves on. The recap goes when the next question
/// reaches the talker; "close that" or "forget that" answered by the core leave it alone.
#[test]
fn the_recap_leaves_when_the_next_question_comes() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        r#"To sum up. <recap bullets="Reads try Redis first|A miss reads Postgres|TTLs bound staleness"/>That's it."#,
    ));
    sim.talker
        .push(ScriptedReply::says("Raft elects a leader."));
    sim.user_says(0, "Sum up the cache for me.");
    sim.run_until(15_000);
    let recap = recap_ids(&sim);
    assert_eq!(recap.len(), 1);
    assert!(
        !dismissed(&sim, recap[0]),
        "it stays while nothing moves on"
    );
    sim.user_says(15_000, "Okay. How does Raft elect a leader?");
    sim.run_until(30_000);
    assert_clean(&sim);
    assert!(dismissed(&sim, recap[0]), "the topic moved on");
    let effects: Vec<&Effect> = sim.effects().map(|(_, effect)| effect).collect();
    let gone = effects
        .iter()
        .position(|effect| matches!(effect, Effect::Ui(UiEvent::SurfaceDismiss { .. })))
        .expect("dismissed");
    let asked = effects
        .iter()
        .rposition(|effect| matches!(effect, Effect::Talk { .. }))
        .expect("asked");
    assert!(gone < asked, "the recap leaves as the question goes out");
}

/// A turn the talker is still answering keeps its own recap: a tool round continues the reply.
/// The recap is on screen before the round's request goes out.
#[test]
fn a_tool_round_keeps_the_recap() {
    let mut sim = Sim::new(config());
    sim.talker.push(
        ScriptedReply::says(concat!(
            r#"<recap bullets="One|Two|Three"/>To sum up, reads try Redis first. "#,
            "A miss reads Postgres and writes the value back. ",
            "The TTL bounds how stale a read can be. ",
            "Eviction keeps the cache within its memory. ",
            "Let me check what is still open for you.",
        ))
        .calls("show_tasks", "{}"),
    );
    sim.talker
        .push(ScriptedReply::says("Three tasks are open."));
    sim.user_says(0, "Sum it up and show my tasks.");
    sim.run_until(30_000);
    assert_clean(&sim);
    let recap = recap_ids(&sim);
    assert_eq!(recap.len(), 1);
    let shown_at = sim
        .ui()
        .find(|(_, event)| matches!(event, UiEvent::SurfaceShow { .. }))
        .map(|(at, _)| at)
        .expect("shown");
    let rounds: Vec<_> = sim
        .effects()
        .filter(|(_, effect)| matches!(effect, Effect::Talk { .. }))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(rounds.len(), 2, "the question and one tool round");
    assert!(
        shown_at < rounds[1],
        "the recap is up when the round's request goes out"
    );
    assert!(!dismissed(&sim, recap[0]));
}

fn libraries_shown(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(
            effect,
            Effect::Ui(UiEvent::ViewShow {
                view: View::Library { .. },
                ..
            })
        )
    })
}

/// D-146: a replay played from the library goes back to it when it closes (the UI closes it once
/// it has played to its end), and the library then closes as usual.
#[test]
fn a_replay_opened_over_the_library_returns_to_it() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Sessions.").calls("show_library", "{}"));
    sim.talker.push(ScriptedReply::says("Here they are."));
    sim.talker
        .push(ScriptedReply::says("Playing it.").calls("play_session", r#"{"id":"raft"}"#));
    sim.talker.push(ScriptedReply::says("It's playing."));
    sim.user_says(0, "show me my sessions");
    sim.run_until(8_000);
    sim.user_says(8_000, "play the Raft one");
    sim.run_until(16_000);
    assert_eq!(sim.core().view(), Some(ViewKind::Replay));
    sim.at(16_000, Input::Command(UiCommand::CloseView));
    sim.run_until(17_000);
    assert_eq!(sim.core().view(), Some(ViewKind::Library));
    assert_eq!(libraries_shown(&sim), 2, "the same library comes back");
    let dismissals = count(&sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::ViewDismiss))
    });
    assert_eq!(dismissals, 0, "the replay gives way to the library");
    sim.at(17_000, Input::Command(UiCommand::CloseView));
    sim.run_until(18_000);
    assert_clean(&sim);
    assert_eq!(sim.core().view(), None);
    assert_eq!(libraries_shown(&sim), 2);
}

/// A replay asked for from the live stage goes back to the live stage.
#[test]
fn a_replay_opened_from_the_stage_returns_to_the_stage() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Playing it.").calls("play_session", r#"{"id":"raft"}"#));
    sim.talker.push(ScriptedReply::says("It's playing."));
    sim.user_says(0, "play the Raft one");
    sim.run_until(8_000);
    assert_eq!(sim.core().view(), Some(ViewKind::Replay));
    sim.user_says(8_000, "close that");
    sim.run_until(12_000);
    assert_clean(&sim);
    assert_eq!(sim.core().view(), None);
    assert_eq!(libraries_shown(&sim), 0);
}

/// Found by the fuzzer: a harness report that comes in while "Session saved." plays must not
/// start a turn once the session has ended (the orchestrator would cut it off at once). The
/// research ends while the user says it: with it still running, BoardAI would ask first.
#[test]
fn a_report_waiting_when_the_session_ends_starts_nothing() {
    let mut sim = Sim::new(config());
    sim.harness.run_ms = 20_000;
    sim.talker
        .push(ScriptedReply::says("I'll look into it.").calls(
            "research",
            r#"{"question":"where is the TTL","depth":"quick"}"#,
        ));
    sim.talker.push(ScriptedReply::says("It's running."));
    sim.user_says(0, "Find where the TTL is set.");
    assert!(sim.run_until_true(20_000, |sim| sim.core().state().name() == "idle"));
    let ran = sim
        .effects()
        .find_map(|(at, effect)| matches!(effect, Effect::RunHarness { .. }).then_some(at))
        .expect("the research ran")
        .as_ms();
    // The report lands half a second into the words.
    let at = ran + 20_000 - 500;
    let before = talks(&sim);
    sim.user_says(at, "Ok, let's stop here.");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(ends(&sim), 1);
    assert_eq!(
        talks(&sim),
        before,
        "the report starts no turn after the end"
    );
}

/// The detour's answer ends on an offer ("Want me to keep going on how it evicts?"): "Yes, keep
/// going." accepts it, so the detour stays open and the talker goes on with it (D-147 closes
/// only when "go on" asks for the main explanation).
#[test]
fn keep_going_after_a_detour_offer_keeps_the_detour() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="cache-aside"/>That's the cache-aside pattern. "#,
        r#"<node id="redis" label="Redis" kind="db"/>Reads go to Redis first. "#,
        "Want me to keep going on how it evicts?",
    )));
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| sim.speech.word_starts().len() > 8));
    let at = sim.now().as_ms();
    sim.user_says_with_onset(at, "wait, why Redis first?");
    sim.run_until(at + 30_000);
    assert_eq!(sim.core().thread().depth(), 1);
    sim.user_says(at + 30_000, "Yes, keep going.");
    sim.run_until(at + 60_000);
    assert_clean(&sim);
    assert_eq!(pops(&sim), 0, "the user said yes to more of the detour");
    assert_eq!(sim.core().thread().depth(), 1);
}

/// "Go back to the main board" over a detour's stroke: the detour closes with its stroke frozen
/// on it, so the talker isn't told that stroke finishes later.
#[test]
fn going_back_over_a_detour_stroke_never_promises_it_finishes() {
    use boardai_core::history::UserPart;
    use boardai_core::protocol::DrawOp;
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="cache-aside"/>That's the cache-aside pattern. "#,
        r#"<node id="redis" label="Redis" kind="db"/>Reads go to Redis first, and only on a miss "#,
        r#"do they go to Postgres, which is slower and holds the truth."#,
    )));
    sim.talker
        .push(ScriptedReply::says("So, back to the request."));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| sim.speech.word_starts().len() > 8));
    let at = sim.now().as_ms();
    sim.user_says_with_onset(at, "wait, why Redis first?");
    assert!(sim.run_until_true(at + 30_000, |sim| {
        sim.ui().any(|(_, event)| {
            matches!(event, UiEvent::OpStart { op: DrawOp::Node { id, .. }, .. } if id.as_str() == "redis")
        })
    }));
    let over = sim.now().as_ms() + 20;
    sim.user_says_with_onset(over, "go back to the main board");
    sim.run_until(over + 30_000);
    assert_clean(&sim);
    assert_eq!(sim.core().thread().depth(), 0);
    let request = sim.talker.requests.last().expect("asked");
    let words = request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                UserPart::Text { text } => Some(text.clone()),
                UserPart::ToolResult { .. } => None,
            }),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .unwrap_or_default();
    assert!(words.contains("go back to the main board"), "{words}");
    assert!(
        !words.contains("of redis"),
        "a closed detour's stroke 'finishes': {words}"
    );
}
