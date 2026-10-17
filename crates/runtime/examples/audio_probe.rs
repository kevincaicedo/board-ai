//! `just audio-probe`: plays two clauses through VoiceProcessingIO with the macOS system voice
//! and prints what speech out reports, so you can hear the voice and see positions advance.
//!
//! Options: `--text "…"` (one clause), `--voice <identifier>`, `--list-voices`,
//! `--list-inputs` (the microphones and whether this process may record; never prompts),
//! `--list-devices` (every device, its directions, and the system defaults),
//! `--input builtin|default|<device name>` (which microphone the unit opens),
//! `--output default|<device name>` (which speaker), and `--watch <seconds>`: open the unit,
//! speak nothing, and print what happens as devices come and go (unplug the chosen microphone,
//! plug in headphones, change the default in System Settings).
#![allow(clippy::print_stdout, reason = "a dev probe prints its findings")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use boardai_core::ids::ClauseId;
use boardai_core::input::Input;
use boardai_runtime::audio::{
    AudioConfig, AudioIo, InputChoice, OutputChoice, device_snapshot, input_device_names,
    microphone_access, run_main_loop_until,
};
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
    output: OutputChoice,
    list_voices: bool,
    list_inputs: bool,
    list_devices: bool,
    /// Watch device changes this long instead of speaking.
    watch: Option<Duration>,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        clauses: DEFAULT_CLAUSES
            .iter()
            .map(|clause| (*clause).to_owned())
            .collect(),
        voice: VoiceId::default(),
        input: InputChoice::SystemDefault,
        output: OutputChoice::SystemDefault,
        list_voices: false,
        list_inputs: false,
        list_devices: false,
        watch: None,
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
                    Some(name) => InputChoice::Named(name.to_owned()),
                    None => return Err("--input needs builtin, default or a name".to_owned()),
                }
            }
            "--output" => {
                options.output = match arguments.next().as_deref() {
                    Some("default") => OutputChoice::SystemDefault,
                    Some(name) => OutputChoice::Named(name.to_owned()),
                    None => return Err("--output needs default or a name".to_owned()),
                }
            }
            "--watch" => {
                let seconds: u64 = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .ok_or("--watch needs a number of seconds")?;
                options.watch = Some(Duration::from_secs(seconds));
            }
            "--list-voices" => options.list_voices = true,
            "--list-inputs" => options.list_inputs = true,
            "--list-devices" => options.list_devices = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn main() -> Result<(), String> {
    let options = options()?;
    if options.list_inputs {
        println!("microphone access: {:?}", microphone_access());
        for name in input_device_names() {
            println!("input: {name}");
        }
        return Ok(());
    }
    if options.list_devices {
        list_devices();
        return Ok(());
    }
    if options.list_voices {
        for voice in system_voices() {
            println!(
                "{}  {} (quality {})",
                voice.identifier, voice.name, voice.quality
            );
        }
        return Ok(());
    }
    let (mut audio, _mic, output) = AudioIo::start(AudioConfig {
        input: options.input.clone(),
        output: options.output.clone(),
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
    let events = audio.take_device_events();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let outcome = match (options.watch, events) {
            (Some(period), Some(events)) => runtime.block_on(watch(period, events)),
            (Some(_), None) => Err("no device events".to_owned()),
            (None, _) => runtime.block_on(speak(options, output)),
        };
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

/// Every device with its directions, and the system defaults.
fn list_devices() {
    let snapshot = device_snapshot();
    for device in &snapshot.devices {
        println!(
            "{:>4}  {}{}{}{}{}",
            device.id.0,
            device.name,
            if device.has_input { "  [in]" } else { "" },
            if device.has_output { "  [out]" } else { "" },
            if device.built_in { "  (built in)" } else { "" },
            if device.virtual_device {
                "  (virtual)"
            } else {
                ""
            },
        );
    }
    println!(
        "default input {:?}, default output {:?}",
        snapshot.default_input.map(|id| id.0),
        snapshot.default_output.map(|id| id.0)
    );
}

/// Prints device events until `period` is over.
async fn watch(
    period: Duration,
    mut events: tokio::sync::mpsc::Receiver<boardai_runtime::audio::DeviceEvent>,
) -> Result<(), String> {
    let started = Instant::now();
    println!("watching devices for {} s", period.as_secs());
    let deadline = tokio::time::sleep(period);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { return Err("the unit stopped".to_owned()) };
                println!("{:>6} ms  {event:?}", started.elapsed().as_millis());
            }
            () = &mut deadline => return Ok(()),
        }
    }
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
