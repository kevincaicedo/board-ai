//! The virtual voice reports like a real speech out, on tokio's paused clock.

use std::time::Duration;

use boardai_core::ids::ClauseId;
use boardai_core::input::{Input, PlaybackPosition};

use super::{VirtualVoice, VirtualVoiceConfig};
use crate::session::SessionChannels;
use crate::session::inbox::Envelope;
use crate::session::ports::SpeechOut;

async fn next_input(channels: &mut SessionChannels) -> Input {
    match channels.receiver.recv().await {
        Some(Envelope::Input(input)) => input,
        other => panic!("expected an input, got {other:?}"),
    }
}

fn position(channels: &SessionChannels) -> Option<PlaybackPosition> {
    *channels.positions.borrow()
}

#[tokio::test(start_paused = true)]
async fn clauses_play_in_order_and_the_drain_follows_the_last() {
    let mut channels = SessionChannels::new();
    let config = VirtualVoiceConfig { ms_per_byte: 10 };
    let mut voice = VirtualVoice::spawn(channels.speech_out(), config);
    assert!(voice.synthesize(ClauseId(0), "one two".into()).is_ok());
    assert!(voice.synthesize(ClauseId(1), "three".into()).is_ok());
    let Input::ClauseReady { clause, timings } = next_input(&mut channels).await else {
        panic!("ClauseReady first");
    };
    assert_eq!(
        (clause, timings.duration_ms, timings.words.len()),
        (ClauseId(0), 70, 2)
    );
    assert!(matches!(
        next_input(&mut channels).await,
        Input::ClauseReady {
            clause: ClauseId(1),
            ..
        }
    ));
    tokio::time::sleep(Duration::from_millis(45)).await;
    let first = position(&channels).map(|at| at.clause);
    assert_eq!(first, Some(ClauseId(0)));
    tokio::time::sleep(Duration::from_millis(60)).await;
    let second = position(&channels).map(|at| at.clause);
    assert_eq!(
        second,
        Some(ClauseId(1)),
        "the second clause follows the first"
    );
    assert_eq!(next_input(&mut channels).await, Input::PlaybackDrained);
    let end = position(&channels);
    assert_eq!(
        end,
        Some(PlaybackPosition {
            clause: ClauseId(1),
            offset_ms: 50
        })
    );
}

#[tokio::test(start_paused = true)]
async fn stop_answers_with_the_last_position_after_the_fade() {
    let mut channels = SessionChannels::new();
    let config = VirtualVoiceConfig { ms_per_byte: 10 };
    let mut voice = VirtualVoice::spawn(channels.speech_out(), config);
    assert!(
        voice
            .synthesize(ClauseId(4), "a fairly long clause to stop inside".into())
            .is_ok()
    );
    assert!(matches!(
        next_input(&mut channels).await,
        Input::ClauseReady { .. }
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let playing = position(&channels).map_or(0, |at| at.offset_ms);
    assert!(voice.stop().is_ok());
    let Input::VoiceStopped { last: Some(last) } = next_input(&mut channels).await else {
        panic!("VoiceStopped with a position");
    };
    assert_eq!(last.clause, ClauseId(4));
    assert!(
        last.offset_ms >= playing + 30,
        "the fade counts as heard: {last:?}"
    );
    assert!(last.offset_ms <= playing + 60, "{last:?}");
    // Nothing else plays: no drain report for a stopped voice.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(channels.receiver.try_recv().is_err());
    assert!(voice.stop().is_ok());
    assert_eq!(
        next_input(&mut channels).await,
        Input::VoiceStopped { last: None }
    );
}

#[test]
fn speed_scales_the_rate_and_is_clamped() {
    assert_eq!(
        VirtualVoiceConfig::with_speed(1.0),
        VirtualVoiceConfig::default()
    );
    let fast = VirtualVoiceConfig::with_speed(2.0).ms_per_byte;
    assert_eq!(fast * 2, VirtualVoiceConfig::default().ms_per_byte + 1);
    assert_eq!(
        VirtualVoiceConfig::with_speed(1_000.0),
        VirtualVoiceConfig::with_speed(crate::limits::VIRTUAL_VOICE_SPEED_MAX)
    );
    assert_eq!(
        VirtualVoiceConfig::with_speed(f32::NAN),
        VirtualVoiceConfig::default()
    );
}
