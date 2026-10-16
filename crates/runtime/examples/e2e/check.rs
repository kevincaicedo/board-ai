//! What the run must show, checked from its record and its replay.
//!
//! Pipeline checks hold whatever the model says: the user is heard, answered and spoken to,
//! strokes follow the voice, a barge-in ducks, stops and commits what was heard. Model checks
//! depend on the talker choosing to open a detour and come back; they fail the run only with
//! `--strict`.

use std::collections::BTreeSet;

use boardai_core::CoreConfig;
use boardai_core::effect::{LogRecord, Milestone};
use boardai_core::history::{AssistantPart, Message, UserPart};
use boardai_core::input::{Input, StopReason};
use boardai_core::protocol::UiEvent;
use boardai_core::sim::check::{squash, strip_tags};
use boardai_core::time::SessionTime;

use crate::clips::Clip;
use crate::latency;
use crate::replay::{Record, Replay, inputs};
use crate::trace::Entry;

/// Barge-in budgets (HLD §8).
const DUCK_BUDGET_MS: i64 = 100;
const STOP_BUDGET_MS: i64 = 400;
/// A transcript is the line when it has this share of the line's words, in percent.
const TRANSCRIPT_WORDS_PERCENT: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Pipeline,
    /// Depends on what the model chooses to say.
    Model,
    /// Measured against a budget, reported, never failing the run: the budgets are targets until
    /// the spikes settle them (HLD §8).
    Budget,
}

struct Check {
    name: &'static str,
    class: Class,
    passed: bool,
    detail: String,
}

fn check(name: &'static str, class: Class, passed: bool, detail: String) -> Check {
    Check {
        name,
        class,
        passed,
        detail,
    }
}

/// Runs every check, prints them and the latencies; true when the run passes.
pub fn report(
    record: &Record,
    clips: &[Clip],
    played: &[(SessionTime, SessionTime)],
    config: &CoreConfig,
    strict: bool,
) -> bool {
    let replay = Replay::run(record, config);
    let mut checks = vec![matches(record, &replay), anchors(&replay)];
    checks.extend(transcripts(record, clips, played));
    checks.push(answers(record, clips.len()));
    checks.push(answers_end(record));
    checks.push(clauses_play(record, clips.len()));
    if let Some(index) = clips.iter().position(Clip::is_barge_in) {
        checks.extend(barge_in(record, &replay, clips, played, index));
    }
    println!();
    for check in &checks {
        let verdict = match (check.passed, check.class) {
            (true, _) => "pass ",
            (false, Class::Pipeline) => "FAIL ",
            (false, Class::Model) => "MODEL",
            (false, Class::Budget) => "OVER ",
        };
        println!("{verdict}  {:<34} {}", check.name, check.detail);
    }
    latency::table(record, &replay, clips, played);
    checks.iter().all(|check| match check.class {
        Class::Pipeline => check.passed,
        Class::Model => check.passed || !strict,
        Class::Budget => true,
    })
}

pub fn marks(record: &Record, wanted: Milestone) -> Vec<SessionTime> {
    record
        .iter()
        .filter_map(|(at, entry)| match entry {
            Entry::Record {
                record: LogRecord::Milestone { milestone },
            } if *milestone == wanted => Some(*at),
            Entry::Input { .. } | Entry::Ui { .. } | Entry::Record { .. } => None,
        })
        .collect()
}

pub fn ui(record: &Record) -> impl Iterator<Item = (SessionTime, &UiEvent)> {
    record.iter().filter_map(|(at, entry)| match entry {
        Entry::Ui { event } => Some((*at, event)),
        Entry::Input { .. } | Entry::Record { .. } => None,
    })
}

pub fn first_at_or_after(times: &[SessionTime], from: SessionTime) -> Option<SessionTime> {
    times.iter().copied().find(|at| *at >= from)
}

/// Where clip `index`'s turn ends: when the next clip starts, or never.
fn turn_end(played: &[(SessionTime, SessionTime)], index: usize) -> SessionTime {
    played
        .get(index + 1)
        .map_or(SessionTime::from_ms(u64::MAX), |(started, _)| *started)
}

pub fn final_transcripts(record: &Record) -> Vec<(SessionTime, String)> {
    inputs(record)
        .filter_map(|(at, input)| {
            let Input::Transcript {
                text,
                is_final: true,
            } = input
            else {
                return None;
            };
            (!text.trim().is_empty()).then(|| (at, text.clone()))
        })
        .collect()
}

fn words(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|ch| ch.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// The live session and the replay produced the same UI events: what ran is exactly what the
/// core decides from the recorded inputs.
fn matches(record: &Record, replay: &Replay) -> Check {
    // `SessionStart` comes from the orchestrator opening a session, not from the core.
    let live: Vec<&UiEvent> = ui(record)
        .map(|(_, event)| event)
        .filter(|event| !matches!(event, UiEvent::SessionStart { .. }))
        .collect();
    let first_difference = live.iter().zip(&replay.ui).position(|(a, b)| *a != b);
    let same = live.len() == replay.ui.len() && first_difference.is_none();
    check(
        "replay rebuilds the session",
        Class::Pipeline,
        same,
        format!(
            "{} live UI events, {} replayed over {} session(s), first difference at \
             {first_difference:?}",
            live.len(),
            replay.ui.len(),
            replay.sessions
        ),
    )
}

fn anchors(replay: &Replay) -> Check {
    let detail = if replay.violations.is_empty() {
        format!(
            "{} ops after their word played ({} ended with their reply, not checked)",
            replay.ops_checked, replay.ops_unanchored
        )
    } else {
        replay.violations.join("; ")
    };
    check(
        "ops fire after their words (I1)",
        Class::Pipeline,
        replay.violations.is_empty() && replay.ops_checked > 0,
        detail,
    )
}

/// Each line was heard: a final transcript in its turn with most of its words.
fn transcripts(
    record: &Record,
    clips: &[Clip],
    played: &[(SessionTime, SessionTime)],
) -> Vec<Check> {
    let finals = final_transcripts(record);
    clips
        .iter()
        .zip(played)
        .enumerate()
        .map(|(index, (clip, at))| {
            let end = turn_end(played, index);
            let heard: Vec<&str> = finals
                .iter()
                .filter(|(when, _)| *when >= at.0 && *when < end)
                .map(|(_, text)| text.as_str())
                .collect();
            let expected = words(&clip.text);
            let found = expected.intersection(&words(&heard.join(" "))).count();
            check(
                "the line is transcribed",
                Class::Pipeline,
                found * 100 >= expected.len() * TRANSCRIPT_WORDS_PERCENT,
                format!("said {:?}, heard {:?}", clip.text, heard.join(" | ")),
            )
        })
        .collect()
}

fn answers(record: &Record, lines: usize) -> Check {
    let requested = marks(record, Milestone::TalkerRequested).len();
    let first_tokens = marks(record, Milestone::TalkerFirstToken).len();
    check(
        "the talker answers",
        Class::Pipeline,
        first_tokens >= lines && requested >= lines,
        format!("{requested} requests, {first_tokens} first tokens for {lines} lines"),
    )
}

/// Answers that stopped at the token limit were cut off. With Ollama's default 4,096-token
/// context and a 3,653-token system prompt and tools, that is the context filling up, not the
/// talker's output budget.
fn answers_end(record: &Record) -> Check {
    let stops: Vec<&StopReason> = inputs(record)
        .filter_map(|(_, input)| {
            let Input::TalkerDone { stop, .. } = input else {
                return None;
            };
            Some(stop)
        })
        .collect();
    let cut = stops
        .iter()
        .filter(|stop| matches!(stop, StopReason::MaxTokens))
        .count();
    check(
        "answers end on their own",
        Class::Model,
        cut == 0,
        format!(
            "{cut} of {} answers stopped at the token limit (a full context window)",
            stops.len()
        ),
    )
}

fn clauses_play(record: &Record, lines: usize) -> Check {
    let count = |wanted: fn(&Input) -> bool| inputs(record).filter(|(_, i)| wanted(i)).count();
    let ready = count(|input| matches!(input, Input::ClauseReady { .. }));
    let failed = count(|input| matches!(input, Input::SynthesisFailed { .. }));
    let positions = count(|input| matches!(input, Input::PlaybackAt { .. }));
    let heard = marks(record, Milestone::FirstAudio).len();
    check(
        "clauses play",
        Class::Pipeline,
        ready > 0 && positions > 0 && heard >= lines,
        format!(
            "{ready} clauses synthesized ({failed} failed), {positions} positions, {heard} \
             answers heard"
        ),
    )
}

/// Milliseconds from `from` to the first of `times` at or after `after`.
fn since(times: &[SessionTime], after: SessionTime, from: SessionTime) -> Option<i64> {
    first_at_or_after(times, after).map(|at| at.as_ms() as i64 - from.as_ms() as i64)
}

/// The barge-in ducked in time, stopped the voice, committed what was heard, and the talker
/// opened a detour and came back from it (in that answer, or after "go on").
fn barge_in(
    record: &Record,
    replay: &Replay,
    clips: &[Clip],
    played: &[(SessionTime, SessionTime)],
    index: usize,
) -> Vec<Check> {
    let from = played[index].0;
    let begin = from.plus_ms(clips[index].speech_begin_ms);
    let duck_ms = since(&marks(record, Milestone::OnsetDuck), from, begin);
    let stop_ms = since(&marks(record, Milestone::BargeInStop), from, begin);
    let stopped = inputs(record)
        .any(|(at, input)| at >= from && matches!(input, Input::VoiceStopped { last: Some(_) }));
    let pushed = ui(record)
        .find(|(at, event)| *at >= from && matches!(event, UiEvent::LayerPush { .. }))
        .map(|(at, _)| at);
    let popped = pushed.is_some_and(|pushed| {
        ui(record).any(|(at, event)| at >= pushed && matches!(event, UiEvent::LayerPop { .. }))
    });
    vec![
        check(
            "barge-in ducks the voice",
            Class::Pipeline,
            duck_ms.is_some(),
            format!("on the onset, {duck_ms:?} ms after the speech began"),
        ),
        check(
            "  … within 100 ms",
            Class::Budget,
            duck_ms.is_some_and(|ms| ms <= DUCK_BUDGET_MS),
            format!("{duck_ms:?} ms"),
        ),
        check(
            "barge-in stops the voice",
            Class::Pipeline,
            stop_ms.is_some() && stopped,
            format!("decided {stop_ms:?} ms after the speech began"),
        ),
        check(
            "  … within 400 ms",
            Class::Budget,
            stop_ms.is_some_and(|ms| ms <= STOP_BUDGET_MS),
            format!("{stop_ms:?} ms"),
        ),
        commits_heard_prefix(record, replay, from),
        check(
            "the talker opens a detour",
            Class::Model,
            pushed.is_some(),
            pushed.map_or_else(String::new, |at| format!("at {} ms", at.as_ms())),
        ),
        check("and resumes from it", Class::Model, popped, String::new()),
    ]
}

/// Case (a): an assistant message holding only what was heard, a prefix of what the talker
/// said; or case (b): the next user message says where the user stopped hearing.
fn commits_heard_prefix(record: &Record, replay: &Replay, from: SessionTime) -> Check {
    let said: String = inputs(record)
        .filter(|(at, _)| *at < from)
        .filter_map(|(_, input)| {
            let Input::TalkerDelta { text } = input else {
                return None;
            };
            Some(text.as_str())
        })
        .collect();
    let said = squash(&strip_tags(&said));
    let heard = replay
        .conversation
        .iter()
        .find_map(|message| match message {
            Message::Assistant { parts, raw: None } => Some(
                parts
                    .iter()
                    .filter_map(|part| match part {
                        AssistantPart::Text { text } => Some(text.as_str()),
                        AssistantPart::ToolUse { .. } => None,
                    })
                    .collect::<String>(),
            ),
            Message::Assistant { .. } | Message::User { .. } | Message::System { .. } => None,
        });
    let note = replay
        .conversation
        .iter()
        .find_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                UserPart::Text { text } => text
                    .lines()
                    .find(|line| line.starts_with("[You were interrupted."))
                    .map(str::to_owned),
                UserPart::ToolResult { .. } => None,
            }),
            Message::Assistant { .. } | Message::System { .. } => None,
        });
    let (passed, detail) = match (heard, note) {
        (Some(heard), _) => {
            let heard = squash(&strip_tags(&heard));
            (
                said.starts_with(&heard),
                format!("committed what was heard: {heard:?}"),
            )
        }
        (None, Some(note)) => (
            true,
            format!("the reply was stored whole; the user turn opens {note}"),
        ),
        (None, None) => (false, "nothing marks where the voice was cut".to_owned()),
    };
    check(
        "commits only what was heard",
        Class::Pipeline,
        passed,
        detail,
    )
}
