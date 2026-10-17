//! Simulator scenarios: the speculative start (D-262, LLD §2.2). Speech in is driven as the real
//! one reports it: an `EarlyFinal` once the speaker has been quiet for 96 ms and the decode is
//! done, then `VoiceResumed` if they go on, or `VoiceEnd` (after the 256 ms hangover) and the same
//! text as the final.
//!
//! - On a hosted talker, a finished early final starts the request; nothing of it shows until
//!   the final confirms the turn, and the answer is the same as without it, only sooner.
//! - Speech that resumes cancels it, and the turn goes on as one turn.
//! - A local talker, an unfinished early final, push-to-talk and the core's own controls never
//!   start one; a mute or the deadline cancels one.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::CoreConfig;
use boardai_core::effect::{Effect, LogRecord, Milestone};
use boardai_core::input::Input;
use boardai_core::limits::SPECULATION_MAX_MS;
use boardai_core::protocol::{DrawAction, MicAction, OrbState, UiCommand, UiEvent};
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Sim, USER_MS_PER_WORD};
use boardai_core::time::SessionTime;
use boardai_core::{MicMode, RequestStart};
use common::{assert_clean, config, count, times, transcript};

/// The runtime's voice detector hangover (`speech_in::vad::HANGOVER_MS`, in 32 ms windows).
const HANGOVER_MS: u64 = 256;
/// When the early final reaches the core after the voice stops: 96 ms of quiet (three windows)
/// and a ~50 ms decode (`spike_turns`).
const EARLY_FINAL_MS: u64 = 146;

fn hosted() -> CoreConfig {
    CoreConfig {
        request_start: RequestStart::OnEarlyFinal,
        ..config()
    }
}

fn words_ms(text: &str) -> u64 {
    USER_MS_PER_WORD * u64::try_from(text.split_whitespace().count()).expect("few words")
}

fn early_final(sim: &mut Sim, at_ms: u64, text: &str) {
    sim.at(
        at_ms,
        Input::EarlyFinal {
            text: text.to_owned(),
        },
    );
}

/// The voice stops at `speech_end`: the early final, then the detector's end and the final.
fn voice_stops(sim: &mut Sim, speech_end: u64, voiced_ms: u64, text: &str) {
    early_final(sim, speech_end + EARLY_FINAL_MS, text);
    let reported = speech_end + HANGOVER_MS;
    sim.at(
        reported,
        Input::VoiceEnd {
            voiced_ms: u32::try_from(voiced_ms).expect("short"),
            silence_ms: u32::try_from(HANGOVER_MS).expect("short"),
        },
    );
    sim.at(
        reported,
        Input::Transcript {
            text: text.to_owned(),
            is_final: true,
        },
    );
}

/// The user says `text` from `at_ms`; returns when the voice stopped.
fn says(sim: &mut Sim, at_ms: u64, text: &str) -> u64 {
    sim.at(at_ms, Input::VoiceStart);
    let speech_end = at_ms + words_ms(text);
    voice_stops(sim, speech_end, words_ms(text), text);
    speech_end
}

fn talks(sim: &Sim) -> Vec<u64> {
    times(sim, |effect| matches!(effect, Effect::Talk { .. }))
        .into_iter()
        .map(SessionTime::as_ms)
        .collect()
}

fn cancels(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::CancelTalk))
}

fn marks(sim: &Sim, wanted: Milestone) -> Vec<u64> {
    times(sim, |effect| {
        matches!(effect, Effect::Log(LogRecord::Milestone { milestone }) if *milestone == wanted)
    })
    .into_iter()
    .map(SessionTime::as_ms)
    .collect()
}

fn first_speech(sim: &Sim) -> Option<u64> {
    times(sim, |effect| matches!(effect, Effect::Synthesize { .. }))
        .first()
        .map(|at| at.as_ms())
}

/// When the orb first left Listening for Thinking or Speaking.
fn first_orb_past_listening(sim: &Sim) -> Option<u64> {
    sim.ui()
        .find(|(_, event)| {
            matches!(
                event,
                UiEvent::Orb {
                    state: OrbState::Thinking | OrbState::Speaking,
                    ..
                }
            )
        })
        .map(|(at, _)| at.as_ms())
}

/// The talker is fast enough that its whole answer is in before the turn is confirmed.
fn fast(sim: &mut Sim) {
    sim.talker.first_token_ms = 40;
    sim.talker.chunk_interval_ms = 5;
}

#[test]
fn a_finished_early_final_starts_the_request_and_nothing_shows_until_the_final() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap."));
    let speech_end = says(&mut sim, 0, "Why Redis first?");
    sim.run_until(10_000);
    assert_clean(&sim);
    let early = speech_end + EARLY_FINAL_MS;
    let confirmed = speech_end + HANGOVER_MS;
    assert_eq!(
        talks(&sim),
        vec![early],
        "one request, sent on the early final"
    );
    assert_eq!(cancels(&sim), 0);
    assert_eq!(marks(&sim, Milestone::SpeculativeRequest), vec![early]);
    assert_eq!(marks(&sim, Milestone::TalkerRequested), vec![confirmed]);
    // The answer was in at `early + 40 + …`, before the turn was confirmed: it shows only then.
    assert_eq!(first_speech(&sim), Some(confirmed));
    assert_eq!(first_orb_past_listening(&sim), Some(confirmed));
    let lines = transcript(&sim);
    assert!(
        lines.iter().any(|line| line == "user: Why Redis first?"),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Because reads are cheap.")),
        "{lines:?}"
    );
}

/// The same turn on a local talker: the same conversation, the request only at the end.
#[test]
fn a_speculative_turn_says_exactly_what_the_turn_would_have() {
    let run = |config: CoreConfig| {
        let mut sim = Sim::new(config);
        fast(&mut sim);
        sim.talker.push(ScriptedReply::says(
            "A request starts at the browser, then the API.",
        ));
        sim.talker.push(ScriptedReply::says("Sure, the cache."));
        let end = says(&mut sim, 0, "How does a request get to the database?");
        says(&mut sim, end + 8_000, "Okay, go on.");
        sim.run_until(30_000);
        assert_clean(&sim);
        (sim.talker.requests.clone(), transcript(&sim), talks(&sim))
    };
    let (local_requests, local_lines, local_talks) = run(config());
    let (hosted_requests, hosted_lines, hosted_talks) = run(hosted());
    assert_eq!(hosted_requests, local_requests);
    assert_eq!(hosted_lines, local_lines);
    let ahead: Vec<u64> = local_talks
        .iter()
        .zip(&hosted_talks)
        .map(|(local, hosted)| local - hosted)
        .collect();
    assert_eq!(ahead, vec![HANGOVER_MS - EARLY_FINAL_MS; 2]);
}

/// A pause mid-question: Whisper closes the first part ("Can you show me?", `spike_turns`) after
/// 96 ms of quiet, then the speaker goes on. The early request is cancelled unseen, and the turn
/// is one turn.
#[test]
fn speech_that_resumes_cancels_the_request_and_the_turn_goes_on() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.talker.push(ScriptedReply::says("Two writers race."));
    sim.at(0, Input::VoiceStart);
    let pause_at = words_ms("Why does it fail");
    early_final(&mut sim, pause_at + EARLY_FINAL_MS, "Why does it fail?");
    sim.at(pause_at + 200, Input::VoiceResumed);
    let whole = "Why does it fail on the second write?";
    let speech_end = pause_at + 200 + words_ms("on the second write");
    voice_stops(&mut sim, speech_end, speech_end, whole);
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(
        talks(&sim),
        vec![pause_at + EARLY_FINAL_MS, speech_end + EARLY_FINAL_MS]
    );
    // Cancelled the moment speech came back, not when the next early final came.
    let cancelled = times(&sim, |effect| matches!(effect, Effect::CancelTalk));
    assert_eq!(cancelled, vec![SessionTime::from_ms(pause_at + 200)]);
    assert_eq!(
        marks(&sim, Milestone::TalkerRequested),
        vec![speech_end + HANGOVER_MS]
    );
    let lines = transcript(&sim);
    let users: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("user:"))
        .collect();
    assert_eq!(
        users,
        vec!["user: Why does it fail on the second write?"],
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("Unheard")),
        "{lines:?}"
    );
    assert_eq!(
        first_orb_past_listening(&sim),
        Some(speech_end + HANGOVER_MS)
    );
}

#[test]
fn a_local_talker_never_starts_early() {
    let mut sim = Sim::new(config());
    fast(&mut sim);
    let speech_end = says(&mut sim, 0, "Why Redis first?");
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![speech_end + HANGOVER_MS]);
    assert!(marks(&sim, Milestone::SpeculativeRequest).is_empty());
}

/// Words that don't sound finished, and words the core answers itself, start nothing early.
#[test]
fn only_a_finished_question_for_the_talker_starts_early() {
    for text in [
        "So the leader and",
        "show me the handler",
        "Don't remember this session.",
    ] {
        let mut sim = Sim::new(hosted());
        fast(&mut sim);
        says(&mut sim, 0, text);
        sim.run_until(10_000);
        assert_clean(&sim);
        assert!(
            marks(&sim, Milestone::SpeculativeRequest).is_empty(),
            "{text}: {:?}",
            talks(&sim)
        );
        assert_eq!(cancels(&sim), 0, "{text}");
    }
}

#[test]
fn push_to_talk_never_starts_early() {
    let mut sim = Sim::new(CoreConfig {
        mic: MicMode::PushToTalk,
        ..hosted()
    });
    fast(&mut sim);
    sim.mic(0, MicAction::TalkDown);
    let speech_end = says(&mut sim, 10, "Why Redis first?");
    sim.mic(speech_end + 400, MicAction::TalkUp);
    sim.run_until(10_000);
    assert_clean(&sim);
    assert!(marks(&sim, Milestone::SpeculativeRequest).is_empty());
    assert_eq!(talks(&sim), vec![speech_end + 400]);
}

/// Muting while the request is unconfirmed cancels it, and nothing of it ever shows.
#[test]
fn a_mute_cancels_an_unconfirmed_request() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.at(0, Input::VoiceStart);
    let speech_end = words_ms("Why Redis first?");
    early_final(&mut sim, speech_end + EARLY_FINAL_MS, "Why Redis first?");
    sim.mic(speech_end + 200, MicAction::Mute);
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![speech_end + EARLY_FINAL_MS]);
    assert_eq!(cancels(&sim), 1);
    assert_eq!(first_speech(&sim), None);
    assert!(!transcript(&sim).iter().any(|line| line.contains("Unheard")));
}

/// Speech in that never follows its early final up: the request is cancelled at the deadline,
/// and the late final still ends the turn the usual way.
#[test]
fn the_deadline_cancels_a_request_nothing_confirms() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap."));
    sim.at(0, Input::VoiceStart);
    let speech_end = words_ms("Why Redis first?");
    let early = speech_end + EARLY_FINAL_MS;
    early_final(&mut sim, early, "Why Redis first?");
    let late = early + SPECULATION_MAX_MS + 100;
    sim.at(
        late,
        Input::VoiceEnd {
            voiced_ms: 900,
            silence_ms: 256,
        },
    );
    sim.at(
        late,
        Input::Transcript {
            text: "Why Redis first?".to_owned(),
            is_final: true,
        },
    );
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![early, late]);
    assert_eq!(cancels(&sim), 1);
    let cancelled = times(&sim, |effect| matches!(effect, Effect::CancelTalk));
    assert_eq!(cancelled[0].as_ms(), early + SPECULATION_MAX_MS);
    assert!(!transcript(&sim).iter().any(|line| line.contains("Unheard")));
}

/// A tool the early request calls runs only once the turn is confirmed.
#[test]
fn a_tool_call_waits_for_the_confirmed_turn() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker
        .push(ScriptedReply::says("Let me look.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says("It has a src folder."));
    let speech_end = says(&mut sim, 0, "What is in the repo?");
    sim.run_until(10_000);
    assert_clean(&sim);
    let runs = times(&sim, |effect| matches!(effect, Effect::RunTool { .. }));
    assert_eq!(
        runs.first().map(|at| at.as_ms()),
        Some(speech_end + HANGOVER_MS)
    );
    assert_eq!(cancels(&sim), 0);
}

/// The turn's own request is the early one only when it is exactly the same: a final that says
/// more than its early final (however speech in came to send it) cancels the early request.
#[test]
fn a_final_that_differs_cancels_the_early_request() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap."));
    sim.at(0, Input::VoiceStart);
    let speech_end = words_ms("Why Redis first?");
    early_final(&mut sim, speech_end + EARLY_FINAL_MS, "Why Redis?");
    let confirmed = speech_end + HANGOVER_MS;
    sim.at(
        confirmed,
        Input::VoiceEnd {
            voiced_ms: 900,
            silence_ms: 256,
        },
    );
    sim.at(
        confirmed,
        Input::Transcript {
            text: "Why Redis first?".to_owned(),
            is_final: true,
        },
    );
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![speech_end + EARLY_FINAL_MS, confirmed]);
    assert_eq!(cancels(&sim), 1);
    let lines = transcript(&sim);
    assert!(
        !lines.iter().any(|line| line.contains("Unheard")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Because reads are cheap.")),
        "{lines:?}"
    );
}

/// No stream runs while the user draws (I14): pressing `D` while a question is unconfirmed
/// cancels its early request, and a question said while drawing never starts one.
#[test]
fn drawing_cancels_an_early_request_and_never_starts_one() {
    let mut sim = Sim::new(hosted());
    fast(&mut sim);
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.at(0, Input::VoiceStart);
    let speech_end = words_ms("Why Redis first?");
    early_final(&mut sim, speech_end + EARLY_FINAL_MS, "Why Redis first?");
    let pressed = speech_end + 200;
    let start = UiCommand::Draw {
        action: DrawAction::Start,
    };
    sim.at(pressed, Input::Command(start));
    sim.run_until(5_000);
    let asked = says(&mut sim, 5_000, "Does this make sense?");
    sim.run_until(10_000);
    assert_clean(&sim);
    let cancelled = times(&sim, |effect| matches!(effect, Effect::CancelTalk));
    assert_eq!(cancelled, vec![SessionTime::from_ms(pressed)]);
    assert_eq!(
        marks(&sim, Milestone::SpeculativeRequest),
        vec![speech_end + EARLY_FINAL_MS],
        "nothing early while drawing"
    );
    assert_eq!(talks(&sim).last(), Some(&(asked + HANGOVER_MS)));
    assert!(!transcript(&sim).iter().any(|line| line.contains("Unheard")));
}
