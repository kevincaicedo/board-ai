//! Simulator scenarios: turn-taking, drawing in sync, captions and the orb (LLD §2.2, §2.4).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::{Effect, Gain};
use boardai_core::input::Input;
use boardai_core::limits::{
    END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS, OP_SPACING_MIN_MS, UTTERANCE_MAX_MS,
};
use boardai_core::protocol::{MicAction, OrbState, Speaker, UiEvent};
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Entry, Sim, USER_MS_PER_WORD};
use boardai_core::{CoreConfig, MicMode};
use common::{DEMO, assert_clean, config, count, op_starts, orbs, times, transcript};

fn talks(sim: &Sim) -> Vec<u64> {
    times(sim, |effect| matches!(effect, Effect::Talk { .. }))
        .iter()
        .map(|at| at.as_ms())
        .collect()
}

#[test]
fn a_plain_explain_turn_draws_in_sync_with_speech() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    // I1 is checked on every op by the simulator.
    assert_clean(&sim);
    let voice_end = 9 * USER_MS_PER_WORD;
    assert_eq!(talks(&sim), vec![voice_end + END_OF_TURN_SILENCE_MS]);
    let starts = op_starts(&sim);
    assert_eq!(starts.len(), 7, "four nodes and three edges");
    for pair in starts.windows(2) {
        let gap = pair[1].0.ms_since(pair[0].0);
        assert!(gap >= u64::from(OP_SPACING_MIN_MS), "ops {gap} ms apart");
    }
    let first_audio = times(&sim, |effect| matches!(effect, Effect::Synthesize { .. }))[0];
    assert!(starts[0].0 > first_audio);
    // Captions show text as written, revealed with playback.
    let captions: Vec<String> = sim
        .ui()
        .filter_map(|(_, event)| {
            if let UiEvent::Caption {
                who: Speaker::Assistant,
                text,
                is_final: true,
            } = event
            {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(captions.first().map(String::as_str), Some("Sure."));
    assert!(
        captions
            .iter()
            .any(|caption| caption.ends_with("three API pods."))
    );
    let orbs = orbs(&sim);
    for state in [
        OrbState::Listening,
        OrbState::Thinking,
        OrbState::Drawing,
        OrbState::Idle,
    ] {
        assert!(orbs.contains(&state), "{state:?} in {orbs:?}");
    }
    assert_eq!(orbs.last(), Some(&OrbState::Idle));
    let history = transcript(&sim);
    assert_eq!(history.len(), 3, "{history:#?}");
    assert!(history[1].starts_with("user: Walk me through"));
    assert!(history[2].starts_with("assistant(stored): <plan"));
}

#[test]
fn the_same_inputs_at_the_same_times_give_the_same_effects() {
    let run = || {
        let mut sim = Sim::new(config());
        sim.talker.push(ScriptedReply::says(DEMO));
        sim.user_says(0, "Walk me through how a request reaches our API.");
        sim.user_says(6_000, "wait, why Redis first?");
        sim.run_until(30_000);
        serde_json::to_string(
            &sim.trace()
                .iter()
                .filter_map(|(at, entry)| match entry {
                    Entry::Effect(effect) => Some((at.as_ms(), effect.clone())),
                    Entry::Input(_) => None,
                })
                .collect::<Vec<_>>(),
        )
        .expect("serializes")
    };
    assert_eq!(run(), run());
}

/// I7: "mm-hm" 3.2 s into the answer ducks the voice and brings it back; nothing stops.
#[test]
fn mm_hm_at_three_point_two_seconds_never_stops_the_voice() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(20_000, |sim| !sim.speech.word_starts().is_empty()));
    let speech_start = sim.speech.word_starts()[0].as_ms();
    sim.user_backchannel(speech_start + 3_200, "mm-hm");
    sim.run_until(30_000);
    assert_clean(&sim);
    assert_eq!(count(&sim, |effect| matches!(effect, Effect::StopVoice)), 0);
    let ducks: Vec<f32> = sim
        .effects()
        .filter_map(|(_, effect)| {
            if let Effect::Duck { gain } = effect {
                Some(gain.value())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ducks, vec![Gain::DUCKED.value(), Gain::FULL.value()]);
    assert_eq!(
        count(&sim, |effect| *effect == Effect::Ui(UiEvent::Acknowledge)),
        1
    );
    assert_eq!(op_starts(&sim).len(), 7, "drawing continued");
    assert_eq!(talks(&sim).len(), 1, "a backchannel is not a turn");
}

/// I7 over the whole lexicon, through the simulator.
#[test]
fn every_backchannel_keeps_the_voice_going() {
    for phrase in boardai_core::conversation::cues::BACKCHANNELS {
        let mut sim = Sim::new(config());
        sim.talker.push(ScriptedReply::says(DEMO));
        sim.user_says(0, "Walk me through it.");
        assert!(sim.run_until_true(20_000, |sim| sim.speech.word_starts().len() > 3));
        let at = sim.now().as_ms() + 100;
        sim.user_backchannel(at, phrase);
        sim.run_until(30_000);
        assert_clean(&sim);
        assert_eq!(
            count(&sim, |effect| matches!(effect, Effect::StopVoice)),
            0,
            "{phrase}"
        );
    }
}

#[test]
fn a_continuation_word_earns_the_grace_period() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "so the leader and");
    sim.run_until(10_000);
    assert_clean(&sim);
    let voice_end = 4 * USER_MS_PER_WORD;
    assert_eq!(talks(&sim), vec![voice_end + END_OF_TURN_GRACE_MS]);
}

#[test]
fn a_pause_inside_the_grace_period_keeps_one_turn() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "so the leader and");
    sim.user_says(4 * USER_MS_PER_WORD + 900, "then the followers?");
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim).len(), 1);
    let history = transcript(&sim);
    let user = history
        .iter()
        .find(|line| line.starts_with("user:"))
        .expect("a user message");
    assert!(
        user.contains("so the leader and then the followers?"),
        "{user}"
    );
}

#[test]
fn the_utterance_cap_ends_the_turn() {
    let mut sim = Sim::new(config());
    sim.user_says_for(0, "this goes on and on", UTTERANCE_MAX_MS + 5_000);
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![UTTERANCE_MAX_MS]);
}

/// Thinking + voice: the request is cancelled and the new words join the same turn.
#[test]
fn speaking_while_boardai_thinks_joins_the_same_turn() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 2_000;
    sim.talker.push(ScriptedReply::says("Unheard."));
    sim.talker
        .push(ScriptedReply::says("Sure. Both paths, then."));
    sim.user_says(0, "show me the read path");
    let first_talk = 4 * USER_MS_PER_WORD + END_OF_TURN_SILENCE_MS;
    sim.user_says(first_talk + 300, "and the write path");
    sim.run_until(15_000);
    assert_clean(&sim);
    let effects: Vec<&Effect> = sim.effects().map(|(_, effect)| effect).collect();
    let cancel = effects
        .iter()
        .position(|effect| **effect == Effect::CancelTalk);
    let second_talk = effects
        .iter()
        .enumerate()
        .filter(|(_, effect)| matches!(effect, Effect::Talk { .. }))
        .nth(1)
        .map(|(index, _)| index);
    assert!(
        cancel.is_some() && cancel < second_talk,
        "CancelTalk precedes the new Talk"
    );
    let history = transcript(&sim);
    let users: Vec<&String> = history
        .iter()
        .filter(|line| line.starts_with("user:"))
        .collect();
    assert_eq!(users.len(), 1, "{history:#?}");
    assert!(users[0].contains("show me the read path and the write path"));
    assert!(!history.iter().any(|line| line.contains("Unheard")));
}

#[test]
fn push_to_talk_ignores_voice_until_the_key_is_held() {
    let mut sim = Sim::new(CoreConfig {
        mic: MicMode::PushToTalk,
        ..config()
    });
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(0, "this is the television");
    sim.run_until(3_000);
    assert!(talks(&sim).is_empty());
    sim.mic(3_000, MicAction::TalkDown);
    sim.user_says(3_050, "walk me through the cache");
    // A pause while the key is held does not end the turn.
    sim.run_until(3_050 + 5 * USER_MS_PER_WORD + 2_000);
    assert!(talks(&sim).is_empty());
    let release = sim.now().as_ms();
    sim.mic(release, MicAction::TalkUp);
    sim.run_until(10_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), vec![release]);
}

#[test]
fn mute_stops_the_voice_and_ignores_the_mic_until_unmuted() {
    let mut sim = Sim::new(config());
    sim.talker.chunk_interval_ms = 200;
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through it.");
    assert!(sim.run_until_true(20_000, |sim| sim.speech.word_starts().len() > 4));
    let muted_at = sim.now().as_ms() + 10;
    sim.mic(muted_at, MicAction::Mute);
    sim.user_says(muted_at + 500, "are you there?");
    sim.run_until(muted_at + 5_000);
    let after: Vec<&Effect> = sim
        .effects()
        .filter(|(at, _)| at.as_ms() >= muted_at)
        .map(|(_, effect)| effect)
        .collect();
    assert!(after.contains(&&Effect::StopVoice));
    assert!(after.contains(&&Effect::CancelTalk));
    assert!(
        !after
            .iter()
            .any(|effect| matches!(effect, Effect::Talk { .. }))
    );
    assert_eq!(orbs(&sim).last(), Some(&OrbState::Muted));
    sim.mic(muted_at + 5_000, MicAction::Unmute);
    sim.user_says(muted_at + 5_100, "okay go on");
    sim.run_until(muted_at + 12_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim).len(), 2);
    let heard = transcript(&sim);
    assert!(
        heard
            .iter()
            .any(|line| line.starts_with("assistant(heard): ")),
        "{heard:#?}"
    );
}

#[test]
fn user_captions_follow_partials_and_finals() {
    let mut sim = Sim::new(config());
    sim.user_says(0, "walk me through the request path");
    sim.run_until(5_000);
    let captions: Vec<(String, bool)> = sim
        .ui()
        .filter_map(|(_, event)| {
            if let UiEvent::Caption {
                who: Speaker::User,
                text,
                is_final,
            } = event
            {
                Some((text.clone(), *is_final))
            } else {
                None
            }
        })
        .collect();
    assert!(captions.first().is_some_and(|(_, is_final)| !is_final));
    assert_eq!(
        captions.last(),
        Some(&("walk me through the request path".to_owned(), true))
    );
    let _ = Input::Tick;
}
