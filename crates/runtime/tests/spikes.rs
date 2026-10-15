//! Spikes (HLD §11) that need model files but no audio hardware: `just spike-s4` and
//! `just spike-s2`. Ignored by default; they read models from the app's data folder (or
//! `BOARDAI_MODELS_DIR`) — fetch them with `just models all`.
#![allow(clippy::expect_used, reason = "spikes fail loudly")]
#![allow(clippy::print_stdout, reason = "spikes print their measurements")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use boardai_core::ids::ClauseId;
use boardai_core::input::{Input, WordTiming};
use boardai_runtime::audio::resample::Resampler;
use boardai_runtime::audio::{MIC_RATE_HZ, MIC_RING_FRAMES, mic_ring};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::models::{ModelId, ModelStore};
use boardai_runtime::speech_in::{
    SileroVad, SpeechIn, SpeechToText, SttError, VAD_WINDOW_FRAMES, WhisperStt,
};
use boardai_runtime::speech_out::{
    ClauseText, KokoroFiles, KokoroVoice, TextToSpeech, TtsChunk, VoiceId,
};
use futures_util::StreamExt;
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters,
};

fn model(id: ModelId) -> PathBuf {
    let root = std::env::var_os("BOARDAI_MODELS_DIR").map_or_else(
        || ModelStore::default_root(Path::new(&std::env::var_os("HOME").expect("HOME"))),
        PathBuf::from,
    );
    let path = root.join(id.file().file_name);
    assert!(
        path.exists(),
        "missing {}: run `just models {}`",
        path.display(),
        id.name()
    );
    path
}

fn scratch() -> PathBuf {
    let folder = std::env::temp_dir().join(format!("boardai-spikes-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("scratch folder");
    folder
}

/// macOS `say`, converted to 16 kHz mono 16-bit WAV with `afconvert`, read back as floats.
fn spoken(text: &str, rate_hz: u32, name: &str) -> Vec<f32> {
    let folder = scratch();
    let aiff = folder.join(format!("{name}.aiff"));
    let wav = folder.join(format!("{name}.wav"));
    let status = Command::new("say")
        .arg("-o")
        .arg(&aiff)
        .arg(text)
        .status()
        .expect("say runs");
    assert!(status.success());
    let format = format!("LEI16@{rate_hz}");
    let status = Command::new("afconvert")
        .args(["-f", "WAVE", "-d", &format, "-c", "1"])
        .arg(&aiff)
        .arg(&wav)
        .status()
        .expect("afconvert runs");
    assert!(status.success());
    read_wav_i16(&std::fs::read(&wav).expect("wav"))
}

/// The samples of a 16-bit PCM WAV's `data` chunk.
fn read_wav_i16(bytes: &[u8]) -> Vec<f32> {
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().expect("4 bytes")) as usize;
        let body = &bytes[offset + 8..(offset + 8 + size).min(bytes.len())];
        if id == b"data" {
            return body
                .chunks_exact(2)
                .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
                .collect();
        }
        offset += 8 + size + size % 2;
    }
    panic!("no data chunk")
}

/// Records when each decode ran and how long it took.
struct Timed<S> {
    inner: S,
    decodes: Arc<Mutex<Vec<(bool, Instant, Duration)>>>,
}

impl<S: SpeechToText> SpeechToText for Timed<S> {
    fn push(&mut self, pcm: &[f32]) {
        self.inner.push(pcm);
    }

    fn partial(&mut self) -> Result<Option<String>, SttError> {
        let started = Instant::now();
        let text = self.inner.partial();
        self.decodes
            .lock()
            .expect("lock")
            .push((false, started, started.elapsed()));
        text
    }

    fn finish(&mut self) -> Result<String, SttError> {
        let started = Instant::now();
        let text = self.inner.finish();
        self.decodes
            .lock()
            .expect("lock")
            .push((true, started, started.elapsed()));
        text
    }

    fn discard(&mut self) {
        self.inner.discard();
    }

    fn set_vocabulary(&mut self, terms: &[String]) {
        self.inner.set_vocabulary(terms);
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

struct S4Result {
    first_partial_ms: Option<f64>,
    /// Between the starts of consecutive partial decodes: the cadence.
    decode_gaps_ms: Vec<f64>,
    /// Between consecutive partials reaching the orchestrator (cadence plus decode jitter).
    partial_gaps_ms: Vec<f64>,
    partial_decode_ms: Vec<f64>,
    final_latency_ms: f64,
    final_decode_ms: f64,
    transcript: String,
}

/// Plays `audio` into the mic ring at real-time pace with 1.2 s of silence after it, and
/// measures what speech in reports.
fn run_s4(whisper: &Path, vocabulary: &[&str], audio: &[f32]) -> S4Result {
    let (mut writer, reader) = mic_ring(MIC_RING_FRAMES);
    let decodes = Arc::new(Mutex::new(Vec::new()));
    let stt = Timed {
        inner: WhisperStt::new(whisper).expect("whisper loads"),
        decodes: Arc::clone(&decodes),
    };
    let vad = SileroVad::new(&model(ModelId::SileroVad)).expect("silero loads");
    let (speech, mut events) =
        SpeechIn::spawn(reader, Box::new(vad), Box::new(stt)).expect("spawn");
    speech.set_vocabulary(vocabulary.iter().map(|term| (*term).to_owned()).collect());
    let recorder = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match events.try_recv() {
                Ok(input) => {
                    let done = matches!(input, Input::Transcript { is_final: true, .. });
                    seen.push((Instant::now(), input));
                    if done {
                        break;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        seen
    });
    let mut stream = vec![0.0_f32; 8_000];
    stream.extend_from_slice(audio);
    stream.extend(std::iter::repeat_n(0.0, MIC_RATE_HZ as usize * 6 / 5));
    let started = Instant::now();
    for (index, window) in stream.chunks(VAD_WINDOW_FRAMES).enumerate() {
        writer.write(window);
        let due = Duration::from_millis(32 * (index as u64 + 1));
        std::thread::sleep(due.saturating_sub(started.elapsed()));
    }
    let seen = recorder.join().expect("recorder");
    speech.shutdown();
    summarize(&seen, &decodes.lock().expect("lock"))
}

fn summarize(seen: &[(Instant, Input)], decodes: &[(bool, Instant, Duration)]) -> S4Result {
    let at = |wanted: fn(&Input) -> bool| {
        seen.iter()
            .find(|(_, input)| wanted(input))
            .map(|(when, _)| *when)
    };
    let start = at(|input| *input == Input::VoiceStart).expect("voice start");
    let end = at(|input| matches!(input, Input::VoiceEnd { .. })).expect("voice end");
    let (final_at, transcript) = seen
        .iter()
        .find_map(|(when, input)| {
            let Input::Transcript {
                text,
                is_final: true,
            } = input
            else {
                return None;
            };
            Some((*when, text.clone()))
        })
        .expect("final transcript");
    let partial_times: Vec<Instant> = seen
        .iter()
        .filter(|(_, input)| {
            matches!(
                input,
                Input::Transcript {
                    is_final: false,
                    ..
                }
            )
        })
        .map(|(when, _)| *when)
        .collect();
    let first_partial_ms = partial_times
        .first()
        .map(|first| milliseconds(*first - start));
    let partial_gaps_ms = partial_times
        .windows(2)
        .map(|pair| milliseconds(pair[1] - pair[0]))
        .collect();
    let partial_decode_ms = decodes
        .iter()
        .filter(|d| !d.0)
        .map(|d| milliseconds(d.2))
        .collect();
    let starts: Vec<Instant> = decodes.iter().filter(|d| !d.0).map(|d| d.1).collect();
    let decode_gaps_ms = starts
        .windows(2)
        .map(|pair| milliseconds(pair[1] - pair[0]))
        .collect();
    let final_decode_ms = decodes
        .iter()
        .filter(|d| d.0)
        .map(|d| milliseconds(d.2))
        .next_back()
        .unwrap_or(0.0);
    S4Result {
        first_partial_ms,
        decode_gaps_ms,
        partial_gaps_ms,
        partial_decode_ms,
        final_latency_ms: milliseconds(final_at - end),
        final_decode_ms,
        transcript,
    }
}

fn stats(values: &[f64]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let max = values.iter().copied().fold(0.0, f64::max);
    format!("mean {mean:.0} / max {max:.0}")
}

const UTTERANCES: [&str; 3] = [
    "Wait, what does the load balancer do?",
    "Can you explain how cache invalidation works when two API pods write to Redis at the same time?",
    "Okay so the request goes through the CDN first, then the load balancer picks one of the three pods, and the pod checks Redis before it ever touches Postgres, right?",
];

#[test]
#[ignore = "spike S4: needs the Whisper and Silero model files; run `just spike-s4`"]
fn spike_s4_whisper_partials_and_final_latency() {
    let vocabulary = ["Redis", "Postgres", "CDN", "load balancer", "API pods"];
    let runs = [
        ("base.en", ModelId::WhisperBaseEn, &[][..]),
        (
            "base.en with vocabulary",
            ModelId::WhisperBaseEn,
            &vocabulary[..],
        ),
        ("small.en", ModelId::WhisperSmallEn, &[][..]),
    ];
    for (label, id, terms) in runs {
        let whisper = model(id);
        println!(
            "\nS4 · Whisper {label} (Metal) · pass: a partial every <= 500 ms, final <= 150 ms after end of turn"
        );
        let mut finals = Vec::new();
        let mut cadence = Vec::new();
        let mut gaps = Vec::new();
        for (index, text) in UTTERANCES.iter().enumerate() {
            let audio = spoken(text, MIC_RATE_HZ, &format!("s4-{index}"));
            let result = run_s4(&whisper, terms, &audio);
            println!(
                "  {:.1} s speech | first partial {:.0} ms after VoiceStart | decode starts every {} ms | delivered every {} ms | partial decode {} ms | final {:.0} ms after VoiceEnd (final decode {:.0} ms; 0 = the early final was used)",
                audio.len() as f64 / f64::from(MIC_RATE_HZ),
                result.first_partial_ms.unwrap_or(f64::NAN),
                stats(&result.decode_gaps_ms),
                stats(&result.partial_gaps_ms),
                stats(&result.partial_decode_ms),
                result.final_latency_ms,
                result.final_decode_ms,
            );
            println!("    said:  {text}\n    heard: {}", result.transcript);
            finals.push(result.final_latency_ms);
            cadence.extend(result.decode_gaps_ms);
            gaps.extend(result.partial_gaps_ms);
        }
        let worst_cadence = cadence.iter().copied().fold(0.0, f64::max);
        let worst_gap = gaps.iter().copied().fold(0.0, f64::max);
        let worst_final = finals.iter().copied().fold(0.0, f64::max);
        println!(
            "  {label}: partial decodes at most {worst_cadence:.0} ms apart ({}; a slot that comes during a decode is skipped), delivered at most {worst_gap:.0} ms apart; worst final {worst_final:.0} ms after VoiceEnd ({})",
            if worst_cadence <= 500.0 + 10.0 {
                "pass"
            } else {
                "FAIL"
            },
            if worst_final <= 150.0 { "pass" } else { "FAIL" },
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Spike S2: Kokoro

const NARRATION: [&str; 5] = [
    "Then the request reaches the load balancer,",
    "which spreads traffic across three API pods.",
    "Each pod checks Redis first, and only on a miss does it query Postgres.",
    "So if two pods write the same key at the same time, the last write wins.",
    "Let's open the getUserById handler in service dot go, line 42.",
];

fn kokoro(model_id: ModelId) -> KokoroVoice {
    KokoroVoice::new(KokoroFiles {
        model: model(model_id),
        voice: model(ModelId::KokoroVoiceHeart),
        lexicon: model(ModelId::MisakiUsGold),
    })
    .expect("kokoro loads")
}

struct Spoken {
    audio: Vec<f32>,
    words: Vec<WordTiming>,
    elapsed: Duration,
}

fn synthesize(runtime: &tokio::runtime::Runtime, voice: &dyn TextToSpeech, text: &str) -> Spoken {
    let clause = ClauseText {
        id: ClauseId(1),
        text: text.to_owned(),
    };
    runtime.block_on(async {
        let started = Instant::now();
        let mut stream = voice.synthesize(&clause, &VoiceId::default(), CancelToken::new());
        let mut spoken = Spoken {
            audio: Vec::new(),
            words: Vec::new(),
            elapsed: Duration::ZERO,
        };
        while let Some(chunk) = stream.next().await {
            match chunk.expect("synthesis") {
                TtsChunk::Audio(audio) => spoken.audio.extend(audio),
                TtsChunk::Words(words) => spoken.words.extend(words),
            }
        }
        spoken.elapsed = started.elapsed();
        spoken
    })
}

/// Word starts (ms) heard by Whisper `base.en` with DTW token timestamps, as normalized words.
fn whisper_word_starts(audio_24k: &[f32]) -> Vec<(String, f64)> {
    let mut resampler = Resampler::new(24_000, MIC_RATE_HZ).expect("ratio");
    let mut audio = Vec::new();
    resampler.push(audio_24k, |sample| audio.push(sample));
    audio.extend(std::iter::repeat_n(0.0, MIC_RATE_HZ as usize));
    let parameters = WhisperContextParameters {
        dtw_parameters: DtwParameters {
            mode: DtwMode::ModelPreset {
                model_preset: DtwModelPreset::BaseEn,
            },
            ..DtwParameters::default()
        },
        ..WhisperContextParameters::default()
    };
    let context = WhisperContext::new_with_params(model(ModelId::WhisperBaseEn), parameters)
        .expect("whisper loads");
    let mut state = context.create_state().expect("state");
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_token_timestamps(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    state.full(params, &audio).expect("decode");
    let mut words: Vec<(String, f64)> = Vec::new();
    for segment in state.as_iter() {
        for index in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(index) else {
                continue;
            };
            let text = token
                .to_str_lossy()
                .map(|t| t.into_owned())
                .unwrap_or_default();
            if text.starts_with("[_") || text.starts_with("<|") {
                continue;
            }
            let start_ms = token.token_data().t_dtw as f64 * 10.0;
            match words.last_mut() {
                Some((word, _)) if !text.starts_with(' ') => word.push_str(&normalize(&text)),
                Some(_) | None => words.push((normalize(&text), start_ms)),
            }
        }
    }
    words.retain(|(word, _)| !word.is_empty());
    words
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Kokoro's word starts paired with Whisper's for the words both agree on, in order.
fn paired_starts(text: &str, kokoro: &[WordTiming], whisper: &[(String, f64)]) -> Vec<(f64, f64)> {
    let mut pairs = Vec::new();
    let mut cursor = 0;
    for timing in kokoro {
        let rest = &text[timing.byte_offset as usize..];
        let written = normalize(rest.split_whitespace().next().unwrap_or_default());
        if let Some(found) = whisper[cursor..]
            .iter()
            .position(|(heard, _)| *heard == written)
        {
            let (_, heard_ms) = whisper[cursor + found];
            pairs.push((f64::from(timing.start_ms), heard_ms));
            cursor += found + 1;
        }
    }
    pairs
}

/// Where the voice first rises out of the lead-in silence: the first 5 ms block above -34 dBFS.
fn energy_onset_ms(audio: &[f32]) -> Option<f64> {
    let block = 120;
    audio
        .chunks(block)
        .position(|chunk| {
            let energy: f32 = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            energy.sqrt() > 0.02
        })
        .map(|index| (index * block) as f64 / 24.0)
}

fn write_wav(path: &Path, audio: &[f32], rate_hz: u32) {
    let data: Vec<u8> = audio
        .iter()
        .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes())
        .collect();
    let mut bytes = Vec::with_capacity(44 + data.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&rate_hz.to_le_bytes());
    bytes.extend_from_slice(&(rate_hz * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    std::fs::write(path, bytes).expect("write wav");
}

fn samples_folder() -> PathBuf {
    let folder = std::env::var_os("BOARDAI_SPIKE_OUT")
        .map_or_else(|| std::env::temp_dir().join("boardai-s2"), PathBuf::from);
    std::fs::create_dir_all(&folder).expect("samples folder");
    folder
}

/// Kokoro's word starts against Whisper's DTW estimates, and each clause's first word against
/// the waveform's own energy onset.
fn print_timing_report(label: &str, errors: &[f64], onset_errors: &[f64]) {
    let mut sorted = errors.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let mean_abs = errors.iter().map(|e| e.abs()).sum::<f64>() / errors.len().max(1) as f64;
    let max_abs = errors.iter().map(|e| e.abs()).fold(0.0, f64::max);
    let within = errors.iter().filter(|e| e.abs() <= 50.0).count();
    let within_corrected = errors
        .iter()
        .filter(|e| (*e - median).abs() <= 50.0)
        .count();
    let onsets: Vec<String> = onset_errors.iter().map(|e| format!("{e:+.0}")).collect();
    println!(
        "  {label}: word starts vs Whisper DTW: mean |Δ| {mean_abs:.0} ms, median Δ {median:.0} ms, max |Δ| {max_abs:.0} ms, {within}/{} within 50 ms ({within_corrected} after removing the median offset)",
        errors.len(),
    );
    println!(
        "  {label}: first word vs the waveform's energy onset, per clause (ms): {}",
        onsets.join(" "),
    );
}

#[test]
#[ignore = "spike S2: needs the Kokoro, lexicon and Whisper model files; run `just spike-s2`"]
fn spike_s2_kokoro_speed_and_timings() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let folder = samples_folder();
    let gap = vec![0.0_f32; 24_000 / 5];
    for (label, id) in [
        ("fp32", ModelId::KokoroModel),
        ("8-bit", ModelId::KokoroModelQuantized),
    ] {
        let voice = kokoro(id);
        println!(
            "\nS2 · Kokoro v1.0 {label}, af_heart · pass: RTF <= 0.3 (on M1), timings within 50 ms, no GPL"
        );
        let warm_up = synthesize(&runtime, &voice, "Warming up.");
        println!("  warm-up: {:.0} ms", milliseconds(warm_up.elapsed));
        let (mut total_audio, mut total_time, mut errors) = (0.0, 0.0, Vec::new());
        let mut onset_errors = Vec::new();
        let mut narration = Vec::new();
        for text in NARRATION {
            let spoken = synthesize(&runtime, &voice, text);
            let audio_s = spoken.audio.len() as f64 / 24_000.0;
            let rtf = spoken.elapsed.as_secs_f64() / audio_s;
            total_audio += audio_s;
            total_time += spoken.elapsed.as_secs_f64();
            let pairs = paired_starts(text, &spoken.words, &whisper_word_starts(&spoken.audio));
            errors.extend(pairs.iter().map(|(kokoro, whisper)| kokoro - whisper));
            let first_word = spoken.words.first().map(|word| f64::from(word.start_ms));
            if let (Some(first_word), Some(onset)) = (first_word, energy_onset_ms(&spoken.audio)) {
                onset_errors.push(first_word - onset);
            }
            println!(
                "  {:.2} s audio in {:.0} ms (RTF {rtf:.3}) · {} words timed, {} matched by Whisper · {text}",
                audio_s,
                milliseconds(spoken.elapsed),
                spoken.words.len(),
                pairs.len(),
            );
            narration.extend_from_slice(&spoken.audio);
            narration.extend_from_slice(&gap);
        }
        let rtf = total_time / total_audio;
        let verdict = if rtf <= 0.3 { "pass" } else { "FAIL" };
        println!("  {label}: RTF {rtf:.3} overall ({verdict})");
        print_timing_report(label, &errors, &onset_errors);
        let path = folder.join(format!("kokoro-af_heart-{}.wav", label.replace(' ', "-")));
        write_wav(&path, &narration, 24_000);
        println!("  sample: {}", path.display());
    }
    let reference = spoken(&NARRATION.join(" "), 24_000, "system-voice-reference");
    let path = folder.join("system-voice-reference.wav");
    write_wav(&path, &reference, 24_000);
    println!("\n  system voice (say) reference: {}", path.display());
}
