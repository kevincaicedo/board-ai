//! The end-of-turn levers, on the clips below (the sentences, commas and pauses measured for
//! D-126 and D-155, and short replies): each clip runs
//! through the real detector and Whisper as speech in runs it (the early final decoded after
//! [`EARLY_FINAL_AFTER_QUIET_MS`] of quiet, timed), and each candidate rule says when the turn
//! would end. A turn that ends before the speaker's last voiced moment is a false end of turn.
//!
//! - **D-126** as it stood: the final's closing mark alone, at `VoiceEnd`.
//! - **Whole replies**: a final that is a whole reply without its mark ("Okay, go on") sounds
//!   finished (D-261).
//! - **A shorter wait**: a finished early final ends the turn after `h` ms of quiet instead of
//!   at `VoiceEnd` (256 ms).
//! - **Speculative start** (D-155 revisited): the request starts on a finished early final and
//!   shows nothing until `VoiceEnd` confirms it; counts the requests wasted.

use std::time::Instant;

use boardai_core::conversation::cues::{
    Completeness, SENTENCE_END_MARKS, completeness, ends_unfinished,
};
use boardai_core::limits::{
    END_OF_TURN_FINISHED_SILENCE_MS, END_OF_TURN_GRACE_MS, END_OF_TURN_SILENCE_MS,
};
use boardai_runtime::audio::MIC_RATE_HZ;
use boardai_runtime::models::ModelId;
use boardai_runtime::speech_in::WhisperStt;
use boardai_runtime::speech_in::vad::{EARLY_FINAL_AFTER_QUIET_MS, VadEvent, VadMachine};

use super::{
    FRAMES_PER_MS, PAUSED, SENTENCES, SPECULATIVE, VOICES, WINDOW_MS, decode, model, percentile,
    probabilities, rms_span_ms, spoken, voice_name,
};

/// Short replies said alone, which Whisper may leave without a closing mark.
const REPLIES: [&str; 8] = [
    "Okay, go on.",
    "Go on.",
    "Keep going.",
    "Yeah.",
    "Mm-hmm.",
    "Go back.",
    "Okay.",
    "Continue.",
];

/// Quiet a finished early final would need to end the turn, in ms, against `VoiceEnd`'s 256.
const SHORTER_WAITS_MS: [u64; 4] = [128, 160, 192, 224];

/// One quiet stretch inside speech, as speech in sees it. Times are ms into the clip.
struct Pause {
    /// The end of the last window at or above the silence threshold.
    last_voiced_ms: u64,
    /// The early final is decoded once quiet reaches 96 ms, and ready after its decode.
    ready_ms: u64,
    early: String,
    /// Speech came back before `VoiceEnd`: the early final is discarded.
    resumed_ms: Option<u64>,
    voice_end_ms: Option<u64>,
    /// After `VoiceEnd`: the next `VoiceStart`, when speech came back in the clip.
    next_start_ms: Option<u64>,
}

struct Run {
    kind: &'static str,
    voice: Option<&'static str>,
    text: &'static str,
    speech_end_ms: u64,
    pauses: Vec<Pause>,
}

impl Run {
    /// The speaker's last voiced moment: a turn decided before it was a false end.
    fn last_voiced_ms(&self) -> u64 {
        self.pauses.last().map_or(0, |pause| pause.last_voiced_ms)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// D-126 before D-261: only the closing mark makes a final finished.
    Marks,
    /// D-126 with whole replies (D-261).
    WholeReplies,
    /// Whole replies, and a finished early final ends the turn after this much quiet.
    Quiet(u64),
}

impl Rule {
    fn name(self) -> String {
        match self {
            Self::Marks => "D-126 (closing marks only)".to_owned(),
            Self::WholeReplies => "+ whole replies (D-261)".to_owned(),
            Self::Quiet(ms) => format!("+ finished early final at {ms} ms of quiet"),
        }
    }

    fn judge(self, text: &str) -> Completeness {
        match self {
            Self::Marks => marks_only(text),
            Self::WholeReplies | Self::Quiet(_) => completeness(text),
        }
    }

    /// When the core would end the turn, if it does in the clip.
    fn decide(self, pauses: &[Pause]) -> Option<u64> {
        for pause in pauses {
            let judged = self.judge(&pause.early);
            if let Self::Quiet(quiet_ms) = self
                && judged == Completeness::Finished
            {
                let at = (pause.last_voiced_ms + quiet_ms).max(pause.ready_ms);
                let still_quiet = pause.resumed_ms.is_none_or(|resumed| at < resumed);
                if still_quiet && pause.voice_end_ms.is_none_or(|end| at <= end) {
                    return Some(at);
                }
            }
            let Some(end) = pause.voice_end_ms else {
                continue;
            };
            let wait_ms = match judged {
                Completeness::Finished => END_OF_TURN_FINISHED_SILENCE_MS,
                Completeness::Unclear => END_OF_TURN_SILENCE_MS,
                Completeness::Unfinished => END_OF_TURN_GRACE_MS,
            };
            let at = end.max(pause.last_voiced_ms + wait_ms);
            if pause.next_start_ms.is_none_or(|start| at < start) {
                return Some(at);
            }
        }
        None
    }
}

/// D-126's completeness before D-261: the closing mark alone.
fn marks_only(text: &str) -> Completeness {
    if ends_unfinished(text) {
        return Completeness::Unfinished;
    }
    let closed = text
        .trim_end()
        .trim_end_matches(['"', '\'', ')', ']', '”', '’']);
    if closed.ends_with(SENTENCE_END_MARKS) {
        Completeness::Finished
    } else {
        Completeness::Unclear
    }
}

/// Runs a clip through the detector, decoding the early final where speech in would.
fn run_clip(whisper: &mut WhisperStt, samples: &[f32], decode_ms: &mut Vec<f64>) -> Vec<Pause> {
    let mut machine = VadMachine::default();
    let mut onset_frame = 0_usize;
    let mut open: Option<Pause> = None;
    let mut pauses: Vec<Pause> = Vec::new();
    for (index, p) in probabilities(samples).iter().enumerate() {
        let event = machine.step(*p);
        let end_ms = (index as u64 + 1) * WINDOW_MS;
        match event {
            Some(VadEvent::Start { onset_frame: frame }) => {
                onset_frame = frame as usize;
                if let Some(last) = pauses.last_mut()
                    && last.voice_end_ms.is_some()
                    && last.next_start_ms.is_none()
                {
                    last.next_start_ms = Some(end_ms);
                }
            }
            Some(VadEvent::End { .. }) => {
                if let Some(mut pause) = open.take() {
                    pause.voice_end_ms = Some(end_ms);
                    pauses.push(pause);
                }
            }
            Some(VadEvent::Onset { .. }) | None => {}
        }
        if !machine.in_speech() {
            continue;
        }
        let quiet_ms = u64::from(machine.quiet_ms());
        if quiet_ms >= u64::from(EARLY_FINAL_AFTER_QUIET_MS) && open.is_none() {
            let after_ms = end_ms as usize - onset_frame / FRAMES_PER_MS;
            let started = Instant::now();
            let early = decode(whisper, samples, onset_frame, after_ms);
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            decode_ms.push(elapsed_ms);
            open = Some(Pause {
                last_voiced_ms: end_ms - quiet_ms,
                ready_ms: end_ms + elapsed_ms.ceil() as u64,
                early,
                resumed_ms: None,
                voice_end_ms: None,
                next_start_ms: None,
            });
        } else if quiet_ms == 0
            && let Some(mut pause) = open.take()
        {
            pause.resumed_ms = Some(end_ms - WINDOW_MS);
            pauses.push(pause);
        }
    }
    pauses
}

fn clip_set() -> Vec<(&'static str, &'static str)> {
    let mut set: Vec<(&str, &str)> = SENTENCES.iter().map(|text| ("sentence", *text)).collect();
    set.extend(SPECULATIVE.iter().map(|(kind, text)| {
        let kind = if *kind == "natural" {
            "natural"
        } else {
            "150-250"
        };
        (kind, *text)
    }));
    set.extend(PAUSED.iter().map(|text| ("900 ms", *text)));
    set.extend(REPLIES.iter().map(|text| ("reply", *text)));
    set
}

const KINDS: [&str; 5] = ["sentence", "natural", "150-250", "900 ms", "reply"];

/// Every rule over every clip: false ends by kind, and how soon a true end is decided.
#[test]
#[ignore = "measurement: needs the Silero and Whisper models; see the module comment"]
fn spike_end_of_turn_levers() {
    let mut whisper = WhisperStt::new(&model(ModelId::WhisperBaseEn)).expect("whisper loads");
    let _ = decode(&mut whisper, &vec![0.0; MIC_RATE_HZ as usize], 8_000, 200);
    let mut decode_ms = Vec::new();
    let mut runs = Vec::new();
    for voice in VOICES {
        for (kind, text) in clip_set() {
            let samples = spoken(text, voice, None);
            let (_, speech_end_ms) = rms_span_ms(&samples);
            let pauses = run_clip(&mut whisper, &samples, &mut decode_ms);
            runs.push(Run {
                kind,
                voice,
                text,
                speech_end_ms,
                pauses,
            });
        }
    }
    println!("\nend-of-turn levers · {} clips", runs.len());
    for run in &runs {
        let line: Vec<String> = run
            .pauses
            .iter()
            .map(|pause| {
                let how = match (pause.resumed_ms, pause.voice_end_ms) {
                    (Some(at), _) => format!("resumed@{}", at - pause.last_voiced_ms),
                    (None, Some(_)) => "end".to_owned(),
                    (None, None) => "open".to_owned(),
                };
                format!(
                    "{how} ready+{} {:?}",
                    pause.ready_ms - pause.last_voiced_ms,
                    pause.early
                )
            })
            .collect();
        println!(
            "  {:<22} {:<8} tail {:>3} · {}",
            voice_name(run.voice),
            run.kind,
            run.last_voiced_ms() as i64 - run.speech_end_ms as i64,
            line.join(" · ")
        );
    }
    let mut rules = vec![Rule::Marks, Rule::WholeReplies];
    rules.extend(SHORTER_WAITS_MS.map(Rule::Quiet));
    for rule in rules {
        report_rule(rule, &runs);
    }
    report_speculation(&runs);
    println!(
        "  early-final decode p50 {:.0} / p90 {:.0} / max {:.0} ms over {}",
        percentile(&decode_ms, 50),
        percentile(&decode_ms, 90),
        percentile(&decode_ms, 100),
        decode_ms.len()
    );
}

fn report_rule(rule: Rule, runs: &[Run]) {
    let mut false_ends = [0_usize; KINDS.len()];
    let mut totals = [0_usize; KINDS.len()];
    let mut latency = Vec::new();
    let mut examples = Vec::new();
    for run in runs {
        let kind = KINDS.iter().position(|kind| *kind == run.kind).unwrap_or(0);
        totals[kind] += 1;
        let Some(at) = rule.decide(&run.pauses) else {
            continue;
        };
        if at < run.last_voiced_ms() {
            false_ends[kind] += 1;
            if examples.len() < 6 && run.kind != "900 ms" {
                examples.push(format!("{} {:?}", voice_name(run.voice), run.text));
            }
        } else {
            latency.push(at as f64 - run.speech_end_ms as f64);
        }
    }
    let counts: Vec<String> = KINDS
        .iter()
        .enumerate()
        .map(|(index, kind)| format!("{kind} {}/{}", false_ends[index], totals[index]))
        .collect();
    println!(
        "  {:<44} false ends: {} · end of turn p50 {:.0} / p90 {:.0} ms",
        rule.name(),
        counts.join(", "),
        percentile(&latency, 50),
        percentile(&latency, 90)
    );
    if !examples.is_empty() {
        println!("      e.g. {}", examples.join("; "));
    }
}

/// D-155 revisited: a request on every finished early final (whole replies counted), shown
/// only when `VoiceEnd` and the final confirm it.
fn report_speculation(runs: &[Run]) {
    let mut started = [0_usize; KINDS.len()];
    let mut wasted = [0_usize; KINDS.len()];
    let mut lead_ms = Vec::new();
    let mut turns = 0;
    for run in runs {
        let kind = KINDS.iter().position(|kind| *kind == run.kind).unwrap_or(0);
        turns += 1;
        let decided = Rule::WholeReplies.decide(&run.pauses);
        for pause in &run.pauses {
            if completeness(&pause.early) != Completeness::Finished {
                continue;
            }
            started[kind] += 1;
            match (pause.resumed_ms, pause.voice_end_ms) {
                (Some(resumed), _) if resumed > pause.ready_ms => wasted[kind] += 1,
                // Resumed before the decode finished: speech in never sends it.
                (Some(_), _) => started[kind] -= 1,
                (None, Some(end)) => {
                    if decided == Some(end) {
                        lead_ms.push(end as f64 - pause.ready_ms as f64);
                    }
                }
                (None, None) => {}
            }
        }
    }
    let counts: Vec<String> = KINDS
        .iter()
        .enumerate()
        .map(|(index, kind)| format!("{kind} {}/{}", wasted[index], started[index]))
        .collect();
    println!(
        "  speculative start: wasted/started {} ({} clips) · request ahead of the confirmed \
         turn p50 {:.0} / min {:.0} / max {:.0} ms over {}",
        counts.join(", "),
        turns,
        percentile(&lead_ms, 50),
        percentile(&lead_ms, 0),
        percentile(&lead_ms, 100),
        lead_ms.len()
    );
}
