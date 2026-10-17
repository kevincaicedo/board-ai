//! The latencies of each turn, stage by stage, against HLD §8. Every time is session time from
//! the record, except where the user's speech begins and ends: that is where the clip's sound
//! went into the mic ring, measured by the file mic.
//!
//! A hosted talker's request may leave on the early final, before the turn is confirmed
//! (D-262): "end of turn" is when the turn was confirmed, "request" when the request that
//! answered it left, and the first token is when its first delta arrived, which may be before
//! the confirmation; the stages from "request" on add up to voice to voice.

use boardai_core::effect::Milestone;
use boardai_core::input::Input;
use boardai_core::time::SessionTime;

use crate::check::{final_transcripts, first_at_or_after, marks};
use crate::clips::Clip;
use crate::replay::{Record, Replay, inputs};

/// One turn's moments; `None` where the run never got there.
struct Turn {
    speech_end: SessionTime,
    voice_end: Option<SessionTime>,
    final_at: Option<SessionTime>,
    /// When the turn was confirmed.
    requested: Option<SessionTime>,
    /// When the request that answered it left: on the early final, or at `requested`.
    sent: Option<SessionTime>,
    first_token: Option<SessionTime>,
    first_clause: Option<SessionTime>,
    first_ready: Option<SessionTime>,
    first_audio: Option<SessionTime>,
}

fn gap(from: Option<SessionTime>, to: Option<SessionTime>) -> String {
    match (from, to) {
        (Some(from), Some(to)) => (to.as_ms() as i64 - from.as_ms() as i64).to_string(),
        _ => "-".to_owned(),
    }
}

fn turns(
    record: &Record,
    replay: &Replay,
    clips: &[Clip],
    played: &[(SessionTime, SessionTime)],
) -> Vec<Turn> {
    let voice_ends: Vec<SessionTime> = inputs(record)
        .filter(|(_, input)| matches!(input, Input::VoiceEnd { .. }))
        .map(|(at, _)| at)
        .collect();
    let readies: Vec<(SessionTime, u32)> = inputs(record)
        .filter_map(|(at, input)| {
            let Input::ClauseReady { clause, .. } = input else {
                return None;
            };
            Some((at, clause.0))
        })
        .collect();
    let finals: Vec<SessionTime> = final_transcripts(record)
        .iter()
        .map(|(at, _)| *at)
        .collect();
    let requested = marks(record, Milestone::TalkerRequested);
    let early = marks(record, Milestone::SpeculativeRequest);
    let resumed: Vec<SessionTime> = inputs(record)
        .filter(|(_, input)| matches!(input, Input::VoiceResumed))
        .map(|(at, _)| at)
        .collect();
    // The stream's first delta as it arrived: held while the turn was unconfirmed, the core's
    // own mark comes only when it is replayed.
    let tokens: Vec<SessionTime> = inputs(record)
        .filter(|(_, input)| {
            matches!(
                input,
                Input::TalkerDelta { .. } | Input::TalkerToolCall { .. }
            )
        })
        .map(|(at, _)| at)
        .collect();
    let audio = marks(record, Milestone::FirstAudio);
    clips
        .iter()
        .zip(played)
        .map(|(clip, at)| {
            let final_at = first_at_or_after(&finals, at.0);
            let request = final_at.and_then(|at| first_at_or_after(&requested, at));
            let sent = request.map(|confirmed| sent_at(&early, &resumed, at.0, confirmed));
            let first_token = sent.and_then(|at| first_at_or_after(&tokens, at));
            let clause = first_token.and_then(|token| {
                replay
                    .synthesized
                    .iter()
                    .find(|(_, sent)| **sent >= token)
                    .map(|(clause, sent)| (clause.0, *sent))
            });
            let first_ready = clause.and_then(|(id, sent)| {
                readies
                    .iter()
                    .find(|(ready, ready_id)| *ready_id == id && *ready >= sent)
                    .map(|(ready, _)| *ready)
            });
            Turn {
                speech_end: at.0.plus_ms(clip.speech_end_ms),
                voice_end: first_at_or_after(&voice_ends, at.0),
                final_at,
                requested: request,
                sent,
                first_token,
                first_clause: clause.map(|(_, sent)| sent),
                first_ready,
                first_audio: request.and_then(|at| first_at_or_after(&audio, at)),
            }
        })
        .collect()
}

/// When the request that answered a line confirmed at `confirmed` left: its last early request
/// since the line began stood unless speech resumed after it.
fn sent_at(
    early: &[SessionTime],
    resumed: &[SessionTime],
    began: SessionTime,
    confirmed: SessionTime,
) -> SessionTime {
    early
        .iter()
        .copied()
        .rfind(|sent| *sent >= began && *sent <= confirmed)
        .filter(|sent| {
            !resumed
                .iter()
                .any(|resume| resume > sent && *resume <= confirmed)
        })
        .unwrap_or(confirmed)
}

/// Prints one row per line, the voice-to-voice median, and how far strokes lag their words.
pub fn table(
    record: &Record,
    replay: &Replay,
    clips: &[Clip],
    played: &[(SessionTime, SessionTime)],
) {
    println!(
        "\nms            end of turn  VoiceEnd→final  request  request→token  token→clause  \
         synthesis  ready→heard  voice→voice"
    );
    println!(
        "HLD §8        200–300      ≤ 150                    300–450        ┐ first audio \
         100–200   ┘            p50 ≤ 1000"
    );
    let mut voice_to_voice = Vec::new();
    for (index, turn) in turns(record, replay, clips, played).iter().enumerate() {
        if let (Some(audio), Some(_)) = (turn.first_audio, turn.requested) {
            voice_to_voice.push(audio.as_ms() as i64 - turn.speech_end.as_ms() as i64);
        }
        println!(
            "line {:<8} {:>11}  {:>14}  {:>7}  {:>13}  {:>12}  {:>9}  {:>11}  {:>11}",
            index + 1,
            gap(Some(turn.speech_end), turn.requested),
            gap(turn.voice_end, turn.final_at),
            gap(Some(turn.speech_end), turn.sent),
            gap(turn.sent, turn.first_token),
            gap(turn.first_token, turn.first_clause),
            gap(turn.first_clause, turn.first_ready),
            gap(turn.first_ready, turn.first_audio),
            gap(Some(turn.speech_end), turn.first_audio),
        );
    }
    voice_to_voice.sort_unstable();
    let median = voice_to_voice.get(voice_to_voice.len() / 2);
    println!(
        "voice to voice p50: {median:?} ms over {} turns",
        voice_to_voice.len()
    );
    let mut lags = replay.stroke_lags_ms.clone();
    lags.sort_unstable();
    println!(
        "strokes after their word (HLD §8 drift ≤ 150): {} ops, p50 {:?} ms, max {:?} ms",
        lags.len(),
        lags.get(lags.len() / 2),
        lags.last()
    );
}
