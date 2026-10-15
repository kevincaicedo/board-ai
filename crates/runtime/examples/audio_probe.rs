//! `just audio-probe`: plays two clauses through VoiceProcessingIO with the macOS system voice
//! and prints what speech out reports, so you can hear the voice and see positions advance.
//!
//! Options: `--text "…"` (one clause), `--voice <identifier>`, `--list-voices`,
//! `--input builtin` (listen on the Mac's own microphone instead of the default input).
#![allow(clippy::print_stdout, reason = "a dev probe prints its findings")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use boardai_core::ids::ClauseId;
use boardai_core::input::Input;
use boardai_runtime::audio::{AudioConfig, AudioIo, InputChoice, run_main_loop_until};
use boardai_runtime::speech_out::{
    SpeechOut, SpeechOutCommand, SpeechOutConfig, SystemVoice, VoiceId, system_voices,
};

const DEFAULT_CLAUSES: [&str; 2] = [
    "Then the request reaches the load balancer,",
    "which spreads traffic across three API pods.",
];

/// Longest the probe waits for the voice to finish.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

struct Options {
    clauses: Vec<String>,
    voice: VoiceId,
    input: InputChoice,
    list_voices: bool,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        clauses: DEFAULT_CLAUSES
            .iter()
            .map(|clause| (*clause).to_owned())
            .collect(),
        voice: VoiceId::default(),
        input: InputChoice::SystemDefault,
        list_voices: false,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--text" => options.clauses = vec![arguments.next().ok_or("--text needs a value")?],
            "--voice" => {
                options.voice = VoiceId::new(arguments.next().ok_or("--voice needs a value")?)
            }
            "--input" => {
                options.input = match arguments.next().as_deref() {
                    Some("builtin") => InputChoice::BuiltIn,
                    Some("default") => InputChoice::SystemDefault,
                    _ => return Err("--input is builtin or default".to_owned()),
                }
            }
            "--list-voices" => options.list_voices = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn main() -> Result<(), String> {
    let options = options()?;
    if options.list_voices {
        for voice in system_voices() {
            println!(
                "{}  {} (quality {})",
                voice.identifier, voice.name, voice.quality
            );
        }
        return Ok(());
    }
    let (audio, _mic, output) = AudioIo::start(AudioConfig {
        input: options.input,
    })
    .map_err(|error| format!("audio: {error}"))?;
    println!(
        "VoiceProcessingIO at {} Hz; output latency {:.1} ms; other-app ducking minimized: {}",
        audio.unit_rate_hz(),
        audio.output_latency_ms(),
        audio.other_audio_ducking_minimized()
    );
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let outcome = runtime.block_on(speak(options, output));
        worker_done.store(true, Ordering::Release);
        outcome
    });
    run_main_loop_until(|| done.load(Ordering::Acquire));
    let outcome = worker
        .join()
        .map_err(|_| "the probe thread panicked".to_owned())?;
    let counters = audio.counters();
    println!(
        "callback counters: capture errors {}, oversized cycles {}, panics {}",
        counters.capture_errors.get(),
        counters.oversized_cycles.get(),
        counters.callback_panics.get()
    );
    outcome
}

async fn speak(
    options: Options,
    output: boardai_runtime::audio::VoiceOutput,
) -> Result<(), String> {
    let engine = SystemVoice::new().map_err(|error| error.to_string())?;
    let config = SpeechOutConfig {
        voice: options.voice,
        fallback: None,
    };
    let (speech, mut channels) = SpeechOut::spawn(Arc::new(engine), config, output);
    let started = Instant::now();
    for (index, text) in options.clauses.iter().enumerate() {
        let clause = ClauseId(u32::try_from(index + 1).unwrap_or(u32::MAX));
        let command = SpeechOutCommand::Synthesize {
            clause,
            text: text.clone(),
        };
        speech
            .sender()
            .send(command)
            .await
            .map_err(|_| "speech out stopped")?;
    }
    let mut playback = channels.playback.clone();
    let mut positions = 0_usize;
    let deadline = tokio::time::sleep(PROBE_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = channels.events.recv() => {
                let Some(event) = event else { break };
                report(&event, started);
                if event == Input::PlaybackDrained {
                    break;
                }
            }
            changed = playback.changed() => {
                if changed.is_err() { break }
                if let Some(position) = *playback.borrow_and_update() {
                    positions += 1;
                    println!(
                        "{:>6} ms  PlaybackAt clause {} offset {} ms",
                        started.elapsed().as_millis(),
                        position.clause.0,
                        position.offset_ms
                    );
                }
            }
            () = &mut deadline => return Err("the voice did not finish in time".to_owned()),
        }
    }
    println!("{positions} positions reported");
    speech.shutdown().await;
    Ok(())
}

fn report(event: &Input, started: Instant) {
    let at = started.elapsed().as_millis();
    let Input::ClauseReady { clause, timings } = event else {
        println!("{at:>6} ms  {event:?}");
        return;
    };
    let words: Vec<(u32, u32)> = timings
        .words
        .iter()
        .map(|word| (word.byte_offset, word.start_ms))
        .collect();
    println!(
        "{at:>6} ms  ClauseReady clause {} ({:?}, {} ms, {} words: {words:?})",
        clause.0,
        timings.source,
        timings.duration_ms,
        words.len(),
    );
}
