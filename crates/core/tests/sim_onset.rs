//! Simulator scenarios: the voice onset's early, tentative duck (LLD §2.2). An onset followed
//! by speech is the barge-in it always was, only ducked earlier; an onset without speech (a
//! cough) restores the gain and changes nothing else; onsets while BoardAI is not speaking do
//! nothing.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::{Effect, Gain, LogRecord, Milestone};
use boardai_core::limits::VOICE_ONSET_DUCK_HOLD_MS;
use boardai_core::protocol::MicAction;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Sim, USER_ONSET_LEAD_MS};
use boardai_core::time::SessionTime;
use boardai_core::{CoreConfig, MicMode};
use common::{DEMO, assert_clean, config, count, times, transcript};

const QUESTION: &str = "Walk me through how a request reaches our API.";

/// A session in the middle of the demo answer, with word `word` playing; returns when it starts.
fn speaking_at_word(word: usize, mic: MicMode) -> (Sim, u64) {
    let mut sim = Sim::new(CoreConfig { mic, ..config() });
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker
        .push(ScriptedReply::says("Because reads are cheap there."));
    if mic == MicMode::PushToTalk {
        sim.mic(0, MicAction::TalkDown);
        sim.mic(2_000, MicAction::TalkUp);
    }
    sim.user_says(0, QUESTION);
    assert!(sim.run_until_true(60_000, |sim| sim.speech.word_starts().len() > word));
    let at = sim.speech.word_starts()[word].as_ms() + 10;
    (sim, at)
}

fn is_onset_duck(effect: &Effect) -> bool {
    *effect
        == Effect::Log(LogRecord::Milestone {
            milestone: Milestone::OnsetDuck,
        })
}

fn ducks(sim: &Sim, gain: Gain) -> Vec<u64> {
    times(sim, |effect| *effect == Effect::Duck { gain })
        .iter()
        .map(|at| at.as_ms())
        .collect()
}

/// Every effect in order, without the wake-ups (an extra tick only moves when something due
/// is noticed, by at most one playback report) and without the onset's own duck, milestone and
/// restore at the given times.
fn decisions(sim: &Sim, onset_at: Option<u64>) -> Vec<Effect> {
    let onset = onset_at.map(SessionTime::from_ms);
    let restore = onset.map(|onset| onset.plus_ms(VOICE_ONSET_DUCK_HOLD_MS));
    sim.effects()
        .filter(|(at, effect)| {
            let wake = matches!(effect, Effect::WakeAt { .. });
            let own_duck = Some(*at) == onset
                && (**effect == Effect::Duck { gain: Gain::DUCKED } || is_onset_duck(effect));
            let own_restore = Some(*at) == restore && **effect == Effect::Duck { gain: Gain::FULL };
            !(wake || own_duck || own_restore)
        })
        .map(|(_, effect)| effect.clone())
        .collect()
}

fn stops(sim: &Sim) -> Vec<SessionTime> {
    times(sim, |effect| *effect == Effect::StopVoice)
}

#[test]
fn an_onset_before_speech_ducks_earlier_and_the_barge_in_goes_on_as_before() {
    let (mut before, at) = speaking_at_word(8, MicMode::OpenMic);
    let (mut with_onset, same_at) = speaking_at_word(8, MicMode::OpenMic);
    assert_eq!(at, same_at, "the simulator is deterministic");
    before.user_says(at, "wait, why Redis first?");
    with_onset.user_says_with_onset(at - USER_ONSET_LEAD_MS, "wait, why Redis first?");
    before.run_until(40_000);
    with_onset.run_until(40_000);
    assert_clean(&before);
    assert_clean(&with_onset);

    let onset = at - USER_ONSET_LEAD_MS;
    assert_eq!(ducks(&before, Gain::DUCKED), vec![at]);
    assert_eq!(
        ducks(&with_onset, Gain::DUCKED),
        vec![onset, at],
        "ducked 160 ms earlier, then the barge-in as before"
    );
    assert_eq!(count(&with_onset, is_onset_duck), 1);
    assert_eq!(
        ducks(&with_onset, Gain::FULL),
        ducks(&before, Gain::FULL),
        "the hold never lifts a duck the barge-in owns"
    );
    // Apart from the onset's own duck, the two sessions decide the same, at the same times.
    assert_eq!(
        decisions(&with_onset, Some(onset)),
        decisions(&before, None)
    );
    assert_eq!(stops(&with_onset), stops(&before));
    assert_eq!(transcript(&with_onset), transcript(&before));
}

#[test]
fn a_cough_restores_the_gain_and_never_stops_the_voice() {
    let (mut quiet, _) = speaking_at_word(8, MicMode::OpenMic);
    let (mut coughed, at) = speaking_at_word(8, MicMode::OpenMic);
    coughed.user_onset(at);
    quiet.run_until(40_000);
    coughed.run_until(40_000);
    assert_clean(&coughed);

    assert_eq!(ducks(&coughed, Gain::DUCKED), vec![at]);
    assert_eq!(
        ducks(&coughed, Gain::FULL),
        vec![at + VOICE_ONSET_DUCK_HOLD_MS]
    );
    assert_eq!(count(&coughed, |effect| *effect == Effect::StopVoice), 0);
    assert!((coughed.speech.gain() - 1.0).abs() < f32::EPSILON);
    // The answer, its drawing and the history are untouched.
    assert_eq!(decisions(&coughed, Some(at)), decisions(&quiet, None));
    assert_eq!(transcript(&coughed), transcript(&quiet));
}

#[test]
fn coughs_in_a_row_extend_the_hold_and_restore_once() {
    let (mut sim, at) = speaking_at_word(8, MicMode::OpenMic);
    sim.user_onset(at);
    sim.user_onset(at + 200);
    sim.user_onset(at + 400);
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(ducks(&sim, Gain::DUCKED), vec![at]);
    assert_eq!(
        ducks(&sim, Gain::FULL),
        vec![at + 400 + VOICE_ONSET_DUCK_HOLD_MS]
    );
    assert_eq!(count(&sim, |effect| *effect == Effect::StopVoice), 0);
}

#[test]
fn a_reply_that_ends_during_the_hold_still_gets_its_gain_back() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.talker.push(ScriptedReply::says("And again."));
    sim.user_says(0, "Say something short.");
    assert!(sim.run_until_true(20_000, |sim| !sim.speech.word_starts().is_empty()));
    let first_word = sim.speech.word_starts()[0].as_ms();
    sim.user_onset(first_word + 100);
    let hold_end = first_word + 100 + VOICE_ONSET_DUCK_HOLD_MS;
    sim.run_until(hold_end - 1);
    // "Sure." lasts 300 ms at the fake voice's pace: the reply ended inside the hold.
    assert_eq!(*sim.core().state(), ConversationState::Idle);
    assert_eq!(ducks(&sim, Gain::FULL), Vec::<u64>::new());
    sim.run_until(hold_end);
    assert_eq!(ducks(&sim, Gain::FULL), vec![hold_end]);
    sim.user_says(hold_end + 500, "Again, please.");
    sim.run_until(30_000);
    assert_clean(&sim);
    assert!((sim.speech.gain() - 1.0).abs() < f32::EPSILON);
}

/// Steps an onset at `at_ms` after checking the state it meets.
fn onset_in(sim: &mut Sim, at_ms: u64, state: &str) {
    sim.run_until(at_ms - 1);
    assert_eq!(sim.core().state().name(), state, "at {at_ms} ms");
    sim.user_onset(at_ms);
    sim.run_until(at_ms);
}

#[test]
fn onsets_while_not_speaking_do_nothing() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 1_500;
    sim.talker.push(ScriptedReply::says("Sure, here it is."));
    sim.user_says(1_000, QUESTION);
    onset_in(&mut sim, 100, "idle");
    onset_in(&mut sim, 1_500, "listening");
    let question_end = 1_000 + 9 * 280;
    onset_in(&mut sim, question_end + 1_000, "thinking");
    sim.run_until(20_000);
    sim.mic(20_000, MicAction::Mute);
    onset_in(&mut sim, 20_500, "muted");
    sim.run_until(21_000);
    assert_clean(&sim);
    assert_eq!(count(&sim, is_onset_duck), 0);
    assert_eq!(
        count(&sim, |effect| matches!(effect, Effect::Duck { .. })),
        0
    );
}

#[test]
fn an_onset_during_a_barge_in_changes_nothing() {
    let (mut sim, at) = speaking_at_word(8, MicMode::OpenMic);
    sim.user_says(at, "wait, why Redis first?");
    onset_in(&mut sim, at + 100, "ducked");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(count(&sim, is_onset_duck), 0);
    assert_eq!(ducks(&sim, Gain::DUCKED), vec![at]);
}

#[test]
fn push_to_talk_ignores_an_onset_without_the_key() {
    let (mut sim, at) = speaking_at_word(8, MicMode::PushToTalk);
    onset_in(&mut sim, at, "speaking");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(count(&sim, is_onset_duck), 0);
    assert_eq!(
        count(&sim, |effect| matches!(effect, Effect::Duck { .. })),
        0
    );
}
