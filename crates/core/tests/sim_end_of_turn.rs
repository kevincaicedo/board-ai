//! Simulator scenarios: when the user's turn ends (LLD §2.2, PRODUCT §6.1). The voice detector
//! here behaves like the real one: it reports `VoiceEnd` only after its 256 ms hangover, says
//! so in `silence_ms`, and the final transcript arrives with it (the early final).
//!
//! - The hangover is silence already heard: it counts once toward the end of the turn.
//! - A final that sounds finished ends the turn as soon as it is in.
//! - A pause in the middle of a sentence ("so the leader… sends a heartbeat") never ends it.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::Effect;
use boardai_core::input::Input;
use boardai_core::limits::{END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS};
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Sim, USER_MS_PER_WORD};
use common::{assert_clean, config, transcript};

/// The runtime's voice detector hangover (`speech_in::vad::HANGOVER_MS`, in 32 ms windows).
const HANGOVER_MS: u64 = 256;

/// The user says `text` from `at_ms` for `voiced_ms`, heard the way the real detector reports
/// it. Returns when the voice stopped (the sound's end).
fn heard(sim: &mut Sim, at_ms: u64, text: &str, voiced_ms: u64) -> u64 {
    sim.at(at_ms, Input::VoiceStart);
    let speech_end = at_ms + voiced_ms;
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
    speech_end
}

fn words_ms(text: &str) -> u64 {
    USER_MS_PER_WORD * u64::try_from(text.split_whitespace().count()).expect("few words")
}

fn talks(sim: &Sim) -> Vec<u64> {
    sim.effects()
        .filter(|(_, effect)| matches!(effect, Effect::Talk { .. }))
        .map(|(at, _)| at.as_ms())
        .collect()
}

fn cancels(sim: &Sim) -> usize {
    sim.effects()
        .filter(|(_, effect)| matches!(effect, Effect::CancelTalk))
        .count()
}

/// The user messages, in order.
fn user_messages(sim: &Sim) -> Vec<String> {
    transcript(sim)
        .into_iter()
        .filter(|line| line.starts_with("user:"))
        .collect()
}

/// T-010 measured 0.9–1.05 s here: the detector's 256 ms hangover, then 600 ms more. A finished
/// question now ends the turn the moment the detector calls the end.
#[test]
fn a_finished_question_ends_the_turn_when_the_detector_calls_the_end() {
    let mut sim = Sim::new(config());
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap."));
    let text = "Why Redis first?";
    let speech_end = heard(&mut sim, 0, text, words_ms(text));
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![speech_end + HANGOVER_MS]);
}

/// Words with no punctuation wait the usual silence, counted once from the voice's end.
#[test]
fn an_unpunctuated_final_waits_the_usual_silence_counted_once() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Sure."));
    let text = "ok go on";
    let speech_end = heard(&mut sim, 0, text, words_ms(text));
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![speech_end + END_OF_TURN_SILENCE_MS]);
}

/// PRODUCT §6.1: "A pause after 'so the leader…' doesn't end the turn." Whisper marks the
/// trailing-off with an ellipsis; a continuation word does the same.
#[test]
fn a_pause_mid_sentence_never_ends_the_turn() {
    // The first parts as Whisper wrote them for `say` with a 900 ms pause (`spike_turns`).
    let cases = [
        ("So the leader…", "sends a heartbeat."),
        ("So the leader...", "Sense a heartbeat to every follower."),
        ("And then the request...", "Go to the load balancer."),
        ("I think the problem is.", "The cache."),
        ("What happens when?", "2 pods right at once."),
        ("The client sends a request, and", "The server answers."),
        ("So the leader and", "the followers vote."),
    ];
    // Just under the grace, counting the hangover the detector already heard.
    let pause_ms = END_OF_TURN_GRACE_MS - 100;
    for (first, rest) in cases {
        let mut sim = Sim::new(config());
        sim.talker.push(ScriptedReply::says("Right."));
        let first_end = heard(&mut sim, 0, first, words_ms(first));
        let resumed_at = first_end + pause_ms;
        let speech_end = heard(&mut sim, resumed_at, rest, words_ms(rest));
        sim.run_until(20_000);
        assert_clean(&sim);
        assert_eq!(
            talks(&sim),
            vec![speech_end + HANGOVER_MS],
            "{first} … {rest}: one turn, ended by the finished second part"
        );
        assert_eq!(cancels(&sim), 0, "{first}: no request was started early");
        let users = user_messages(&sim);
        assert_eq!(users.len(), 1, "{users:?}");
        assert!(users[0].contains(rest), "{users:?}");
    }
}

/// A fragment with no punctuation survives a pause shorter than the usual silence.
#[test]
fn an_unpunctuated_fragment_survives_a_short_pause() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Right."));
    let first_end = heard(&mut sim, 0, "So the leader", words_ms("So the leader"));
    let rest = "sends a heartbeat.";
    let speech_end = heard(&mut sim, first_end + 500, rest, words_ms(rest));
    sim.run_until(20_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![speech_end + HANGOVER_MS]);
    assert_eq!(cancels(&sim), 0);
}

/// The worst case the words allow: Whisper closes a fragment with a period, so the turn ends
/// and the talker is asked. When the user goes on, the request is cancelled before anything was
/// heard and the words join the same turn: nothing is lost and nothing is answered twice.
#[test]
fn a_fragment_closed_with_a_period_rejoins_when_the_user_goes_on() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 1_500;
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.talker
        .push(ScriptedReply::says("Right, every follower."));
    let first_end = heard(&mut sim, 0, "So the leader.", words_ms("So the leader."));
    let rest = "sends a heartbeat.";
    heard(&mut sim, first_end + 700, rest, words_ms(rest));
    sim.run_until(20_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim).len(), 2);
    assert_eq!(talks(&sim)[0], first_end + HANGOVER_MS);
    assert_eq!(cancels(&sim), 1);
    let users = user_messages(&sim);
    assert_eq!(users.len(), 1, "{users:?}");
    assert!(
        users[0].contains("So the leader. sends a heartbeat."),
        "{users:?}"
    );
    assert!(!transcript(&sim).iter().any(|line| line.contains("Unheard")));
}
