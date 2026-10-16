//! Simulator scenarios: interruptions, commit-what-was-heard, detours and resume (LLD §2.6,
//! §2.7).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::{Dropped, Effect, Gain, LogRecord};
use boardai_core::history::{AssistantPart, Message, UserPart};
use boardai_core::ids::{ElementId, LayerId};
use boardai_core::input::Input;
use boardai_core::limits::DETOUR_DEPTH_MAX;
use boardai_core::narration::DETOUR_REFUSAL_LINE;
use boardai_core::protocol::{MicAction, PointerTarget, UiCommand, UiEvent};
use boardai_core::sim::check::{squash, strip_tags};
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Entry, Sim, USER_FINAL_DELAY_MS, USER_MS_PER_WORD};
use common::{DEMO, assert_clean, config, count, op_starts, transcript};

/// Runs until word `word` (1-based) of the answer has started, then interrupts inside it with a
/// cue word. Returns the scripted speech's words, for comparison.
fn interrupt_at_word(sim: &mut Sim, word: usize) {
    assert!(sim.run_until_true(60_000, |sim| sim.speech.word_starts().len() > word));
    let starts = sim.speech.word_starts();
    let (begin, next) = (starts[word - 1].as_ms(), starts[word].as_ms());
    assert!(
        next - begin > 120,
        "word {word} is long enough to stop inside"
    );
    sim.at(begin + 10, Input::VoiceStart);
    sim.at(
        begin + 60,
        Input::Transcript {
            text: "wait".into(),
            is_final: false,
        },
    );
    sim.at(
        begin + 700,
        Input::VoiceEnd {
            voiced_ms: 690,
            silence_ms: 0,
        },
    );
    sim.at(
        begin + 820,
        Input::Transcript {
            text: "wait, why Redis first?".into(),
            is_final: true,
        },
    );
}

fn assistant_texts(sim: &Sim) -> Vec<(String, bool)> {
    sim.core()
        .conversation()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Assistant { parts, raw } => {
                let text: String = parts
                    .iter()
                    .filter_map(|part| match part {
                        AssistantPart::Text { text } => Some(text.as_str()),
                        AssistantPart::ToolUse { .. } => None,
                    })
                    .collect();
                Some((text, raw.is_some()))
            }
            Message::User { .. } | Message::System { .. } => None,
        })
        .collect()
}

fn user_texts(sim: &Sim) -> Vec<String> {
    sim.core()
        .conversation()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                UserPart::Text { text } => Some(text.clone()),
                UserPart::ToolResult { .. } => None,
            }),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .collect()
}

/// Case (a): the generation is still streaming when the user cuts in at word 9. The assistant
/// message holds exactly the nine heard words (the ninth partly) and the drawn tags, nothing else.
#[test]
fn an_interruption_at_word_nine_commits_nine_words_while_streaming() {
    let mut sim = Sim::new(config());
    sim.talker.chunk_interval_ms = 400;
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap there."));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    interrupt_at_word(&mut sim, 9);
    sim.run_until(40_000);
    assert_clean(&sim);
    assert!(count(&sim, |effect| *effect == Effect::CancelTalk) >= 1);
    let assistants = assistant_texts(&sim);
    let (heard, stored) = &assistants[0];
    assert!(!stored, "the cut reply has no provider content");
    let words = strip_tags(heard);
    assert_eq!(words.split_whitespace().count(), 9, "{heard:?}");
    assert!(squash(&strip_tags(DEMO)).starts_with(&squash(&words)));
    assert!(
        heard.contains(r#"<node id="client""#),
        "drawn tags stay: {heard}"
    );
    assert!(
        !heard.contains(r#"<node id="cdn""#),
        "undrawn tags go: {heard}"
    );
    let users = user_texts(&sim);
    assert!(users[1].starts_with("wait, why Redis first?"), "{users:#?}");
    assert!(!users[1].contains("You were interrupted"));
}

/// Case (b): the reply was already complete and stored; it stays untouched, and the next user
/// message says where the user stopped hearing it.
#[test]
fn an_interruption_at_word_nine_after_the_reply_is_stored_adds_a_note() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap there."));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    interrupt_at_word(&mut sim, 9);
    sim.run_until(40_000);
    assert_clean(&sim);
    let assistants = assistant_texts(&sim);
    assert_eq!(
        assistants[0],
        (DEMO.to_owned(), true),
        "stored exactly as returned"
    );
    let users = user_texts(&sim);
    let note = users[1].lines().next().expect("a note line");
    let quoted = note
        .split('"')
        .nth(1)
        .expect("the note quotes the heard text");
    assert_eq!(quoted.split_whitespace().count(), 9, "{note}");
    assert!(note.starts_with("[You were interrupted. The user heard you up to: "));
    assert!(note.ends_with("Nothing after that was heard.]"));
    assert!(users[1].contains("wait, why Redis first?"));
    assert!(
        users[1].contains("[Drawing stopped at"),
        "the frozen op is reported"
    );
}

/// "wait" in 280 ms has no partial: its final decides, and the voice is never brought back up
/// first (it used to be acknowledged as a backchannel, then stopped by the late final).
#[test]
fn a_cue_said_too_fast_for_a_partial_still_interrupts() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through it.");
    assert!(sim.run_until_true(20_000, |sim| sim.speech.word_starts().len() > 5));
    let at = sim.now().as_ms() + 50;
    sim.user_says(at, "wait");
    sim.run_until(30_000);
    assert_clean(&sim);
    let stops: Vec<u64> = sim
        .effects()
        .filter(|(_, effect)| **effect == Effect::StopVoice)
        .map(|(at, _)| at.as_ms())
        .collect();
    assert_eq!(stops.len(), 1);
    assert_eq!(
        count(&sim, |effect| *effect == Effect::Ui(UiEvent::Acknowledge)),
        0
    );
}

/// Rule (d): pointing interrupts as soon as the words are known not to be a backchannel. "this"
/// in 280 ms has no partial, so the decision comes with its final transcript.
#[test]
fn pointing_while_speaking_interrupts_on_the_first_words() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through it.");
    assert!(sim.run_until_true(30_000, |sim| op_starts(sim).len() >= 2));
    let at = sim.now().as_ms() + 10;
    let target = PointerTarget::Element {
        layer: LayerId::MAIN,
        id: ElementId::parse("cdn").expect("valid"),
    };
    sim.at(
        at,
        Input::Command(UiCommand::Pointer {
            target: Some(target),
        }),
    );
    sim.user_says(at + 100, "this");
    sim.run_until(40_000);
    assert_clean(&sim);
    let stop = sim
        .effects()
        .find(|(_, effect)| **effect == Effect::StopVoice)
        .map(|(at, _)| at.as_ms());
    let final_at = at + 100 + USER_MS_PER_WORD + USER_FINAL_DELAY_MS;
    assert_eq!(
        stop,
        Some(final_at),
        "rule (d) decides on the final transcript"
    );
    let users = user_texts(&sim);
    assert!(
        users[1].contains("[Pointing at: cdn \"CDN\" on the main board]"),
        "{users:#?}"
    );
}

#[test]
fn push_to_talk_during_speech_stops_without_ducking() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through it.");
    assert!(sim.run_until_true(30_000, |sim| sim.speech.word_starts().len() > 4));
    let at = sim.now().as_ms() + 10;
    sim.mic(at, MicAction::TalkDown);
    sim.run_until(at + 100);
    let stop = sim
        .effects()
        .find(|(_, effect)| **effect == Effect::StopVoice)
        .map(|(at, _)| at.as_ms());
    assert_eq!(stop, Some(at));
    assert_eq!(
        count(&sim, |effect| *effect
            == Effect::Duck { gain: Gain::DUCKED }),
        0
    );
    assert_clean(&sim);
}

/// Nested detours up to three; a fourth is refused out loud and logged (I4).
#[test]
fn detours_nest_three_deep_and_a_fourth_is_refused() {
    let mut sim = Sim::new(config());
    for title in ["one", "two", "three", "four"] {
        let text = format!(r#"<detour title="{title}"/>Here is detour {title}."#);
        sim.talker.push(ScriptedReply::says(&text));
    }
    let mut at = 0;
    for question in ["what is one?", "and two?", "and three?", "and four?"] {
        sim.user_says(at, question);
        at += 8_000;
    }
    sim.run_until(at + 8_000);
    assert_clean(&sim);
    assert_eq!(sim.core().thread().depth(), DETOUR_DEPTH_MAX);
    let pushes = sim
        .ui()
        .filter(|(_, event)| matches!(event, UiEvent::LayerPush { .. }))
        .count();
    assert_eq!(pushes, DETOUR_DEPTH_MAX);
    let refusal = count(
        &sim,
        |effect| matches!(effect, Effect::Synthesize { text, .. } if text == DETOUR_REFUSAL_LINE),
    );
    assert_eq!(refusal, 1);
    let logged = count(&sim, |effect| {
        *effect
            == Effect::Log(LogRecord::Dropped {
                what: Dropped::Detour,
                count: 1,
            })
    });
    assert_eq!(logged, 1);
}

/// Resume pops the detour, finishes the frozen stroke first, then the bridge's ops continue.
#[test]
fn resume_finishes_the_frozen_op_first() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="cache-aside"/>That's the cache-aside pattern. "#,
        r#"<node id="redis" label="Redis" kind="db"/>Reads go to Redis first. "#,
        r#"<resume/>So, back to the request. "#,
        r#"<node id="pg" label="Postgres" kind="db" near="client" dir="below"/>Then Postgres."#,
    )));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    // Interrupt while the first stroke (the client, with a long budget) is still drawing.
    assert!(sim.run_until_true(30_000, |sim| !op_starts(sim).is_empty()));
    let at = sim.now().as_ms() + 200;
    sim.mic(at, MicAction::TalkDown);
    sim.at(at + 50, Input::VoiceStart);
    sim.at(
        at + 800,
        Input::VoiceEnd {
            voiced_ms: 750,
            silence_ms: 0,
        },
    );
    sim.mic(at + 850, MicAction::TalkUp);
    sim.at(
        at + 900,
        Input::Transcript {
            text: "why Redis first?".into(),
            is_final: true,
        },
    );
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    let events: Vec<(u64, &UiEvent)> = sim.ui().map(|(at, event)| (at.as_ms(), event)).collect();
    let position = |matches: &dyn Fn(&UiEvent) -> bool| {
        events
            .iter()
            .position(|(_, event)| matches(event))
            .expect("event present")
    };
    let freeze =
        position(&|event| matches!(event, UiEvent::Freeze { layer } if *layer == LayerId::MAIN));
    let push = position(&|event| matches!(event, UiEvent::LayerPush { .. }));
    let pop = position(&|event| matches!(event, UiEvent::LayerPop { .. }));
    let resume = position(
        &|event| matches!(event, UiEvent::Resume { layer, .. } if *layer == LayerId::MAIN),
    );
    let postgres = position(
        &|event| matches!(event, UiEvent::OpStart { op: boardai_core::protocol::DrawOp::Node { id, .. }, .. } if id.as_str() == "pg"),
    );
    assert!(freeze < push && push < pop && pop < resume && resume < postgres);
    let UiEvent::Resume { budget_ms, .. } = events[resume].1 else {
        panic!("resume");
    };
    let finished = events[resume].0 + u64::from(*budget_ms);
    assert!(
        events[postgres].0 >= finished,
        "the frozen stroke finishes first"
    );
    let history = transcript(&sim);
    assert!(
        history
            .iter()
            .any(|line| line.contains("[Drawing stopped at")),
        "{history:#?}"
    );
}

/// Interrupting while the voice waits for the talker's next clause stops nothing (nothing is
/// playing), but the duck must still be undone: the next answer is heard at full volume.
#[test]
fn an_interruption_between_clauses_leaves_the_next_answer_at_full_volume() {
    let mut sim = Sim::new(config());
    // One short clause, then the stream stalls for 3 s before its next one.
    sim.talker.chunk_bytes = 64;
    sim.talker.chunk_interval_ms = 3_000;
    sim.talker.push(ScriptedReply::says(
        "Sure, here goes. Then much later, more.",
    ));
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap."));
    sim.user_says(0, "Walk me through it.");
    assert!(sim.run_until_true(30_000, |sim| {
        !sim.speech.word_starts().is_empty() && !sim.speech.is_active()
    }));
    let at = sim.now().as_ms() + 100;
    sim.user_says(at, "wait, why Redis first?");
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    assert!(
        count(&sim, |effect| *effect
            == Effect::Duck { gain: Gain::DUCKED })
            >= 1,
        "the barge-in ducked"
    );
    assert!((sim.speech.gain() - 1.0).abs() < f32::EPSILON);
}

/// Review finding 4 (fixed by D-088): an interruption confirmed while the voice was silent (a
/// tool round was running) never restored the duck, so every later answer played at 25 %.
#[test]
fn the_gain_is_restored_after_an_interruption_over_a_silent_voice() {
    let mut sim = Sim::new(config());
    sim.tools.respond(
        "search_code",
        boardai_core::tools::ToolOutput::Text {
            text: "no matches".into(),
        },
    );
    sim.talker
        .push(ScriptedReply::says("Let me look.").calls("search_code", r#"{"query":"ttl"}"#));
    sim.talker.push(ScriptedReply::says("Found nothing yet."));
    sim.talker.push(ScriptedReply::says(
        "Sure, the cache layer keeps hot rows in memory.",
    ));
    sim.user_says(0, "where is the ttl set");
    assert!(sim.run_until_true(20_000, |sim| sim.talker.requests.len() == 1));
    // The continuation after the tool round is slow to start.
    sim.talker.first_token_ms = 5_000;
    // "Let me look." has played and the voice is silent.
    assert!(sim.run_until_true(20_000, |sim| {
        sim.trace()
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Input(Input::PlaybackDrained)))
    }));
    let at = sim.now().as_ms() + 5;
    sim.user_says(at, "what about the cache layer instead");
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    assert!(
        (sim.speech.gain() - 1.0).abs() < f32::EPSILON,
        "the voice stays ducked"
    );
}

/// Review finding 10: a cue said too fast for a partial, while no clause is ready (between
/// clauses), still interrupts: its final decides, so it is never taken for a backchannel first.
#[test]
fn a_late_cue_interrupts_even_between_clauses() {
    let mut sim = Sim::new(config());
    sim.talker.chunk_interval_ms = 400;
    sim.talker.push(ScriptedReply::says(
        "Sure. The cache sits in front of the database and answers most of the reads.",
    ));
    sim.user_says(0, "explain the cache");
    // "Sure." plays, then the voice waits for the next clause while the talker streams.
    assert!(sim.run_until_true(20_000, |sim| {
        sim.trace()
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Input(Input::PlaybackDrained)))
    }));
    let at = sim.now().as_ms() + 5;
    // "wait" in 300 ms: no partial, an empty transcript at VoiceEnd, the final 120 ms later.
    sim.user_says_for(at, "wait", 300);
    sim.run_until(at + 20_000);
    assert_clean(&sim);
    let acknowledged = sim
        .effects()
        .any(|(_, effect)| *effect == Effect::Ui(UiEvent::Acknowledge));
    assert!(!acknowledged, "never taken for a backchannel");
    let stopped = sim
        .effects()
        .any(|(at_effect, effect)| at_effect.as_ms() > at && *effect == Effect::CancelTalk);
    assert!(
        stopped,
        "the late \"wait\" was ignored: {:#?}",
        transcript(&sim)
    );
}

/// Review finding 23: the UI sends `Pointer` on hover, and rule (d) turned any backchannel said
/// within 1.5 s of the mouse crossing a node into an interruption (I7).
#[test]
fn a_hover_does_not_turn_a_backchannel_into_an_interruption() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "how does a request reach our API");
    assert!(sim.run_until_true(30_000, |sim| !op_starts(sim).is_empty()));
    let at = sim.now().as_ms() + 200;
    // The mouse drifts across the browser node, then the user says "mm-hm".
    sim.at(
        at,
        Input::Command(UiCommand::Pointer {
            target: Some(PointerTarget::Element {
                layer: LayerId::MAIN,
                id: ElementId::parse("client").expect("valid"),
            }),
        }),
    );
    sim.user_backchannel(at + 300, "mm-hm");
    sim.run_until(at + 2_000);
    assert_clean(&sim);
    let stopped = sim
        .effects()
        .any(|(_, effect)| matches!(effect, Effect::StopVoice | Effect::CancelTalk));
    assert!(
        !stopped,
        "a backchannel stopped the voice because the mouse moved"
    );
}
