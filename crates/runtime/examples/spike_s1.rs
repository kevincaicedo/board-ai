//! `just spike-s1`: the echo-cancellation spike (HLD §11 S1). BoardAI's system voice plays
//! through VoiceProcessingIO on the speakers for N minutes while the VAD listens on the same
//! unit's microphone. In a quiet room every `VoiceStart` is a false barge-in: BoardAI hearing
//! itself. Pass: fewer than 1 per 10 minutes.
//!
//! Needs speakers (not headphones), a quiet room, and microphone permission for the terminal.
//! Options: `--minutes N` (default 10), `--input builtin` (the Mac's own microphone, for when
//! the default input is a virtual device), `--voice <identifier>`.
#![allow(clippy::print_stdout, reason = "a spike prints its measurements")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use boardai_core::ids::ClauseId;
use boardai_core::input::Input;
use boardai_runtime::audio::{AudioConfig, AudioIo, InputChoice, VoiceOutput, run_main_loop_until};
use boardai_runtime::models::{ModelId, ModelStore};
use boardai_runtime::speech_in::{SileroVad, SpeechIn, WhisperStt};
use boardai_runtime::speech_out::{
    SpeechOut, SpeechOutCommand, SpeechOutConfig, SystemVoice, VoiceId,
};

const NARRATION: [&str; 8] = [
    "Then the request reaches the load balancer,",
    "which spreads traffic across three API pods.",
    "Each pod checks Redis first, and only on a miss does it query Postgres.",
    "So if two pods write the same key at the same time, the last write wins.",
    "That is why the cache entry carries a version number.",
    "When the version is older than the one in the database, the write is dropped.",
    "Hold on to that idea, because the queue works the same way.",
    "Now let's look at what happens when a pod crashes halfway through a request.",
];

/// Clauses queued ahead of playback, so the voice rarely pauses.
const QUEUED_CLAUSES: u32 = 3;

/// What a false start's transcript says it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heard {
    /// Mostly BoardAI's own words: an echo the canceller let through.
    Echo,
    /// Speech that is not the narration: someone in the room, another app's audio.
    OtherSpeech,
    /// Nothing recognizable: residual echo or noise, counted against the canceller.
    Blank,
}

/// Echo when at least half of the heard words are narration words.
fn classify(transcript: &str) -> Heard {
    let words = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .map(|word| {
                word.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect()
            })
            .filter(|word: &String| !word.is_empty() && word != "blankaudio")
            .collect()
    };
    let narration: std::collections::BTreeSet<String> =
        NARRATION.iter().flat_map(|clause| words(clause)).collect();
    let heard = words(transcript);
    if heard.is_empty() {
        return Heard::Blank;
    }
    let echoed = heard
        .iter()
        .filter(|word| narration.contains(*word))
        .count();
    if echoed * 2 >= heard.len() {
        Heard::Echo
    } else {
        Heard::OtherSpeech
    }
}

struct Options {
    minutes: f64,
    input: InputChoice,
    voice: VoiceId,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        minutes: 10.0,
        input: InputChoice::SystemDefault,
        voice: VoiceId::default(),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--minutes" => {
                let value = arguments.next().ok_or("--minutes needs a value")?;
                options.minutes = value.parse().map_err(|_| "--minutes takes a number")?;
            }
            "--input" => {
                options.input = match arguments.next().as_deref() {
                    Some("builtin") => InputChoice::BuiltIn,
                    Some("default") => InputChoice::SystemDefault,
                    _ => return Err("--input is builtin or default".to_owned()),
                }
            }
            "--voice" => {
                options.voice = VoiceId::new(arguments.next().ok_or("--voice needs a value")?)
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn model(id: ModelId) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let root = std::env::var_os("BOARDAI_MODELS_DIR").map_or_else(
        || ModelStore::default_root(&PathBuf::from(home)),
        PathBuf::from,
    );
    let path = root.join(id.file().file_name);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "missing {}: run `just models {}`",
            path.display(),
            id.name()
        ))
    }
}

fn main() -> Result<(), String> {
    let options = options()?;
    let vad = SileroVad::new(&model(ModelId::SileroVad)?).map_err(|error| error.to_string())?;
    let whisper =
        WhisperStt::new(&model(ModelId::WhisperBaseEn)?).map_err(|error| error.to_string())?;
    let (audio, mic, output) = AudioIo::start(AudioConfig {
        input: options.input,
    })
    .map_err(|error| error.to_string())?;
    println!(
        "S1 · {:.1} min of system voice through VoiceProcessingIO ({} Hz, output latency {:.1} ms) · pass: < 1 false VoiceStart per 10 min",
        options.minutes,
        audio.unit_rate_hz(),
        audio.output_latency_ms()
    );
    let (speech_in, inputs) =
        SpeechIn::spawn(mic, Box::new(vad), Box::new(whisper)).map_err(|e| e.to_string())?;
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let outcome = runtime.block_on(run(options, output, inputs));
        worker_done.store(true, Ordering::Release);
        outcome
    });
    run_main_loop_until(|| done.load(Ordering::Acquire));
    let outcome = worker
        .join()
        .map_err(|_| "the spike thread panicked".to_owned())?;
    speech_in.shutdown();
    let counters = audio.counters();
    println!(
        "audio callbacks: capture errors {}, oversized cycles {}, panics {}",
        counters.capture_errors.get(),
        counters.oversized_cycles.get(),
        counters.callback_panics.get()
    );
    outcome
}

/// What the run has seen so far.
#[derive(Default)]
struct Tally {
    false_starts: u32,
    heard: Vec<Heard>,
    spoken_ms: u64,
}

impl Tally {
    /// Counts a speech-in input; prints false starts and what the recognizer made of them.
    fn speech_in(&mut self, input: Input, started: Instant) {
        let at = started.elapsed().as_secs_f64();
        if input == Input::VoiceStart {
            self.false_starts += 1;
            println!("{at:>7.1} s  false VoiceStart #{}", self.false_starts);
        } else if let Input::Transcript {
            text,
            is_final: true,
        } = input
        {
            let kind = classify(&text);
            self.heard.push(kind);
            println!("{at:>7.1} s    it heard ({kind:?}): {text:?}");
        }
    }

    fn report(&self, minutes: f64) {
        let count = |kind: Heard| self.heard.iter().filter(|h| **h == kind).count();
        let (echo, other, blank) = (
            count(Heard::Echo),
            count(Heard::OtherSpeech),
            count(Heard::Blank),
        );
        // A start whose transcript never came is counted against the canceller too.
        let unexplained = (self.false_starts as usize).saturating_sub(echo + other + blank);
        let per_ten = (echo + blank + unexplained) as f64 * 10.0 / minutes;
        println!(
            "S1: {} VoiceStarts in {minutes:.1} min ({:.1} min of voice synthesized): {echo} echo, {blank} blank, {unexplained} without a transcript, {other} other speech in the room",
            self.false_starts,
            self.spoken_ms as f64 / 60_000.0,
        );
        let quiet = if other > 0 {
            "; the room was not quiet, rerun with no other audio playing"
        } else {
            ""
        };
        let verdict = if per_ten < 1.0 { "pass" } else { "FAIL" };
        println!("S1: {per_ten:.2} self-triggered starts per 10 min ({verdict}){quiet}");
    }
}

async fn run(
    options: Options,
    output: VoiceOutput,
    mut inputs: tokio::sync::mpsc::Receiver<Input>,
) -> Result<(), String> {
    let engine = SystemVoice::new().map_err(|error| error.to_string())?;
    let config = SpeechOutConfig {
        voice: options.voice,
        fallback: None,
    };
    let (speech, mut channels) = SpeechOut::spawn(Arc::new(engine), config, output);
    let sender = speech.sender();
    let started = Instant::now();
    let length = Duration::from_secs_f64(options.minutes * 60.0);
    let (mut next_clause, mut queued) = (0_u32, 0_u32);
    let mut tally = Tally::default();
    let mut progress = tokio::time::interval(Duration::from_secs(30));
    while started.elapsed() < length {
        while queued < QUEUED_CLAUSES {
            let text = NARRATION[next_clause as usize % NARRATION.len()].to_owned();
            next_clause += 1;
            let command = SpeechOutCommand::Synthesize {
                clause: ClauseId(next_clause),
                text,
            };
            sender
                .send(command)
                .await
                .map_err(|_| "speech out stopped")?;
            queued += 1;
        }
        tokio::select! {
            event = channels.events.recv() => {
                let Some(event) = event else { break };
                if let Input::ClauseReady { timings, .. } = event {
                    tally.spoken_ms += u64::from(timings.duration_ms);
                    queued = queued.saturating_sub(1);
                }
            }
            input = inputs.recv() => {
                let Some(input) = input else { break };
                tally.speech_in(input, started);
            }
            _ = progress.tick() => {
                let at = started.elapsed().as_secs_f64();
                println!("{at:>7.1} s  {} false starts so far", tally.false_starts);
            }
        }
    }
    drop(sender);
    speech.shutdown().await;
    tally.report(started.elapsed().as_secs_f64() / 60.0);
    Ok(())
}
