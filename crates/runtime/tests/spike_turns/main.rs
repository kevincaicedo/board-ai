//! Turn-taking measurements (HLD §8, LLD §2.2, §3.4): `say` speech through the real Silero VAD
//! and Whisper `base.en`, offline (no audio hardware, no real time). They settle the numbers the
//! end-of-turn and barge-in rules stand on:
//!
//! - **Voice end:** where the detector's last speech window falls against the sound's own end,
//!   and the silent gaps inside sentences (what a shorter hangover would split).
//! - **Backchannels:** the voiced length the detector reports for every phrase of the lexicon,
//!   at several voices and rates (barge-in rule (c) must stay above all of them, I7), and what an
//!   early Whisper partial makes of them.
//! - **End-of-turn levers** (`levers`): when each candidate rule would end the turn, and how
//!   often too early.
//! - **Cue words:** how soon after the onset a Whisper partial hears an interruption cue, and
//!   what a decode costs on the GPU and on the CPU, idle or while Ollama generates.
//!
//! Ignored by default; they read models from the app's data folder (or `BOARDAI_MODELS_DIR`):
//! `cargo test -p boardai-runtime --test spike_turns -- --ignored --nocapture --test-threads=1`.
//! `BOARDAI_SPIKE_OLLAMA=gemma4:latest` keeps Ollama generating during the cue-word decodes.
#![allow(clippy::expect_used, reason = "spikes fail loudly")]
#![allow(clippy::print_stdout, reason = "spikes print their measurements")]

mod levers;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use boardai_core::conversation::cues::{
    BACKCHANNELS, Completeness, completeness, has_interruption_cue, is_backchannel,
};
use boardai_runtime::audio::MIC_RATE_HZ;
use boardai_runtime::limits::SPEECH_PREROLL_MS;
use boardai_runtime::models::{ModelId, ModelStore};
use boardai_runtime::speech_in::vad::{SILENCE_THRESHOLD, VadEvent, VadMachine};
use boardai_runtime::speech_in::{
    SileroVad, SpeechToText, VAD_WINDOW_FRAMES, VoiceDetector, WhisperDevice, WhisperStt,
};

const WINDOW_MS: u64 = 32;
const FRAMES_PER_MS: usize = MIC_RATE_HZ as usize / 1_000;
/// Silence before each clip, as the e2e run lets the detector settle.
const LEAD_IN_MS: usize = 500;
/// The e2e clips' own measure of where speech is: a 10 ms window above this RMS.
const SPEECH_RMS: f32 = 0.02;

/// The voices `say` speaks with: the system default and four others, female and male, US and UK.
const VOICES: [Option<&str>; 5] = [
    None,
    Some("Daniel"),
    Some("Fred"),
    Some("Reed (English (US))"),
    Some("Flo (English (US))"),
];

fn model(id: ModelId) -> PathBuf {
    let root = std::env::var_os("BOARDAI_MODELS_DIR").map_or_else(
        || ModelStore::default_root(Path::new(&std::env::var_os("HOME").expect("HOME"))),
        PathBuf::from,
    );
    let path = root.join(id.file().file_name);
    assert!(path.exists(), "missing {}", path.display());
    path
}

/// `say` in `voice` at `rate_wpm` (its default when `None`), as 16 kHz mono floats with the
/// lead-in silence before it and a second of silence after it.
fn spoken(text: &str, voice: Option<&str>, rate_wpm: Option<u32>) -> Vec<f32> {
    let folder = std::env::temp_dir().join(format!("boardai-turns-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("scratch folder");
    let aiff = folder.join("clip.aiff");
    let wav = folder.join("clip.wav");
    let mut say = Command::new("say");
    if let Some(voice) = voice {
        say.args(["-v", voice]);
    }
    if let Some(rate) = rate_wpm {
        say.args(["-r", &rate.to_string()]);
    }
    assert!(
        say.arg("-o")
            .arg(&aiff)
            .arg(text)
            .status()
            .expect("say")
            .success()
    );
    let format = format!("LEI16@{MIC_RATE_HZ}");
    let status = Command::new("afconvert")
        .args(["-f", "WAVE", "-d", &format, "-c", "1"])
        .arg(&aiff)
        .arg(&wav)
        .status()
        .expect("afconvert");
    assert!(status.success());
    let mut samples = vec![0.0; LEAD_IN_MS * FRAMES_PER_MS];
    samples.extend(read_wav_i16(&std::fs::read(&wav).expect("wav")));
    samples.extend(std::iter::repeat_n(0.0, MIC_RATE_HZ as usize));
    samples
}

fn read_wav_i16(bytes: &[u8]) -> Vec<f32> {
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let size =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().expect("4 bytes")) as usize;
        let body = &bytes[offset + 8..(offset + 8 + size).min(bytes.len())];
        if &bytes[offset..offset + 4] == b"data" {
            return body
                .chunks_exact(2)
                .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
                .collect();
        }
        offset += 8 + size + size % 2;
    }
    panic!("no data chunk")
}

/// Silero's probability for every 32 ms window, from a fresh detector.
fn probabilities(samples: &[f32]) -> Vec<f32> {
    let mut vad = SileroVad::new(&model(ModelId::SileroVad)).expect("silero loads");
    samples
        .chunks_exact(VAD_WINDOW_FRAMES)
        .map(|window| {
            let window: &[f32; VAD_WINDOW_FRAMES] = window.try_into().expect("a window");
            vad.probability(window).expect("probability")
        })
        .collect()
}

/// Milliseconds where the sound's speech begins and ends (10 ms RMS windows).
fn rms_span_ms(samples: &[f32]) -> (u64, u64) {
    let window = 10 * FRAMES_PER_MS;
    let loud: Vec<usize> = samples
        .chunks(window)
        .enumerate()
        .filter(|(_, chunk)| {
            let energy = chunk.iter().map(|sample| sample * sample).sum::<f32>();
            (energy / chunk.len() as f32).sqrt() > SPEECH_RMS
        })
        .map(|(index, _)| index)
        .collect();
    let first = *loud.first().expect("speech");
    let last = *loud.last().expect("speech");
    (first as u64 * 10, (last as u64 + 1) * 10)
}

/// The detector's events with the window each came in.
fn vad_events(probabilities: &[f32]) -> Vec<(usize, VadEvent)> {
    let mut machine = VadMachine::default();
    probabilities
        .iter()
        .enumerate()
        .filter_map(|(index, p)| machine.step(*p).map(|event| (index, event)))
        .collect()
}

fn percentile(values: &[f64], percent: usize) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
        .get((sorted.len().saturating_sub(1)) * percent / 100)
        .copied()
        .unwrap_or(f64::NAN)
}

fn voice_name(voice: Option<&str>) -> &str {
    voice.unwrap_or("default")
}

const SENTENCES: [&str; 6] = [
    "How does a request get from the browser to the database?",
    "Wait, why Redis first?",
    "Okay, go on.",
    "Can you explain how cache invalidation works when two pods write at once?",
    "So the leader, after the election, sends a heartbeat to every follower.",
    "Show me the handler, then the test.",
];

/// Where the detector ends speech against the sound, and the gaps a shorter hangover would cut,
/// with the silence threshold in use and with none below the speech threshold.
#[test]
#[ignore = "measurement: needs the Silero model; see the module comment"]
fn spike_voice_end_tail_and_inner_gaps() {
    let mut clips = Vec::new();
    for voice in VOICES {
        for text in SENTENCES {
            let samples = spoken(text, voice, None);
            let (_, rms_end) = rms_span_ms(&samples);
            clips.push((voice, text, rms_end, probabilities(&samples)));
        }
    }
    for threshold in [SILENCE_THRESHOLD, 0.5] {
        println!(
            "\nvoice end · silence below {threshold}: tail = the last window at or above it after the sound's end"
        );
        let mut tails = Vec::new();
        let mut gaps = Vec::new();
        for (voice, text, rms_end, p) in &clips {
            let last_voiced = p.iter().rposition(|p| *p >= threshold).expect("speech");
            let tail = ((last_voiced as u64 + 1) * WINDOW_MS) as f64 - *rms_end as f64;
            tails.push(tail);
            let first_voiced = p.iter().position(|p| *p >= 0.5).expect("speech");
            let inner = silent_runs_ms(&p[first_voiced..=last_voiced], threshold);
            gaps.extend(inner.iter().map(|gap| *gap as f64));
            println!(
                "  {:<22} tail {tail:>4.0} ms · inner gaps {inner:?} ms · {text}",
                voice_name(*voice)
            );
        }
        println!(
            "  tail p50 {:.0} / p90 {:.0} / max {:.0} ms over {} clips",
            percentile(&tails, 50),
            percentile(&tails, 90),
            percentile(&tails, 100),
            tails.len()
        );
        for hangover in [128_u64, 160, 192, 224, 256] {
            let splits = gaps.iter().filter(|gap| **gap >= hangover as f64).count();
            println!(
                "  a {hangover} ms hangover would split at {splits} of {} inner gaps",
                gaps.len()
            );
        }
    }
}

/// Lengths of runs of windows below `threshold`, in ms.
fn silent_runs_ms(probabilities: &[f32], threshold: f32) -> Vec<u64> {
    let mut runs = Vec::new();
    let mut run = 0;
    for p in probabilities {
        if *p < threshold {
            run += 1;
        } else if run > 0 {
            runs.push(run * WINDOW_MS);
            run = 0;
        }
    }
    runs
}

/// One backchannel clip as the detector and Whisper report it.
struct Backchannel {
    phrase: &'static str,
    voice: Option<&'static str>,
    rate_wpm: Option<u32>,
    voiced_ms: Vec<u32>,
    final_text: String,
    /// Early partials (at these ms after the onset) that read as an interruption cue.
    cue_misfires: Vec<(u64, String)>,
    /// Every early partial, to try candidate cue words against.
    early: Vec<(u64, String)>,
}

/// Words Whisper's earliest partials make of a cue (`way`, `hold`, `action` at 256 ms), tried
/// as cues before any joins the lexicon: each must misread no backchannel clip.
const CANDIDATE_CUES: [&str; 7] = ["way", "hold", "hold up", "hang", "action", "actual", "we"];

/// The candidate cue among the first `BARGE_IN_CUE_WORDS` words of `text`, if any, matched as
/// the core matches cues (words lowercased, punctuation and contractions dropped).
fn candidate_cue(text: &str) -> Option<&'static str> {
    use boardai_core::conversation::cues::words;
    let words = words(text);
    let first = &words[..words.len().min(3)];
    CANDIDATE_CUES.into_iter().find(|cue| {
        let parts: Vec<&str> = cue.split(' ').collect();
        (0..first.len()).any(|index| {
            parts.iter().enumerate().all(|(offset, part)| {
                first
                    .get(index + offset)
                    .is_some_and(|word| word.split('\'').next() == Some(*part))
            })
        })
    })
}

/// Every backchannel at five voices and three rates: the voiced length the core will see in
/// `VoiceEnd`, and whether Whisper (final, and early partials) keeps it a backchannel.
#[test]
#[ignore = "measurement: needs the Silero and Whisper models; see the module comment"]
fn spike_backchannel_lengths() {
    let mut whisper = WhisperStt::new(&model(ModelId::WhisperBaseEn)).expect("whisper loads");
    let mut clips = Vec::new();
    for phrase in BACKCHANNELS {
        for voice in VOICES {
            for rate_wpm in [Some(150), None, Some(220)] {
                clips.push(measure_backchannel(&mut whisper, phrase, voice, rate_wpm));
            }
        }
    }
    clips.sort_by_key(|clip| clip.voiced_ms.iter().copied().max().unwrap_or(0));
    println!("\nbackchannels · voiced ms per VoiceEnd (several = the detector split it)");
    for clip in &clips {
        println!(
            "  {:>12?} ms  {:<10} {:<22} {:>4}  heard {:?}{}{}",
            clip.voiced_ms,
            clip.phrase,
            voice_name(clip.voice),
            clip.rate_wpm
                .map_or_else(|| "def".to_owned(), |rate| rate.to_string()),
            clip.final_text,
            if is_backchannel(&clip.final_text) {
                ""
            } else {
                "  NOT A BACKCHANNEL"
            },
            if clip.cue_misfires.is_empty() {
                String::new()
            } else {
                format!("  EARLY PARTIAL CUE {:?}", clip.cue_misfires)
            },
        );
    }
    let lengths: Vec<f64> = clips
        .iter()
        .flat_map(|clip| clip.voiced_ms.iter().map(|ms| f64::from(*ms)))
        .collect();
    let totals: Vec<f64> = clips
        .iter()
        .map(|clip| f64::from(clip.voiced_ms.iter().sum::<u32>()))
        .collect();
    report_candidate_misreads(&clips);
    println!(
        "  per VoiceEnd: p50 {:.0} / p90 {:.0} / p99 {:.0} / max {:.0} ms; per clip summed: max {:.0} ms; {} clips",
        percentile(&lengths, 50),
        percentile(&lengths, 90),
        percentile(&lengths, 99),
        percentile(&lengths, 100),
        percentile(&totals, 100),
        clips.len()
    );
}

/// How many backchannel clips each candidate cue, and the lexicon's cues, would misread.
fn report_candidate_misreads(clips: &[Backchannel]) {
    for cue in CANDIDATE_CUES {
        let misread: Vec<String> = clips
            .iter()
            .filter_map(|clip| {
                let (at, text) = clip
                    .early
                    .iter()
                    .find(|(_, text)| candidate_cue(text) == Some(cue))?;
                Some(format!(
                    "{} {}@{at}:{text:?}",
                    clip.phrase,
                    voice_name(clip.voice)
                ))
            })
            .collect();
        println!(
            "  candidate cue {cue:?}: misreads {} of {} clips {misread:?}",
            misread.len(),
            clips.len()
        );
    }
    let misfires = clips
        .iter()
        .filter(|clip| !clip.cue_misfires.is_empty())
        .count();
    println!(
        "  the lexicon's cues misread {misfires} of {} clips",
        clips.len()
    );
}

/// Early partial times checked for cue misfires on backchannels, in ms after the onset.
const EARLY_PARTIALS_MS: [u64; 7] = [192, 224, 256, 288, 320, 352, 384];

fn measure_backchannel(
    whisper: &mut WhisperStt,
    phrase: &'static str,
    voice: Option<&'static str>,
    rate_wpm: Option<u32>,
) -> Backchannel {
    let samples = spoken(phrase, voice, rate_wpm);
    let events = vad_events(&probabilities(&samples));
    let voiced_ms: Vec<u32> = events
        .iter()
        .filter_map(|(_, event)| match event {
            VadEvent::End { voiced_ms, .. } => Some(*voiced_ms),
            VadEvent::Onset { .. } | VadEvent::Start { .. } => None,
        })
        .collect();
    let onset = events.iter().find_map(|(_, event)| match event {
        VadEvent::Start { onset_frame } => Some(*onset_frame as usize),
        VadEvent::Onset { .. } | VadEvent::End { .. } => None,
    });
    let mut cue_misfires = Vec::new();
    let mut early = Vec::new();
    let mut final_text = String::new();
    if let Some(onset) = onset {
        for after_ms in EARLY_PARTIALS_MS {
            let text = decode(whisper, &samples, onset, after_ms as usize);
            if has_interruption_cue(&text) {
                cue_misfires.push((after_ms, text.clone()));
            }
            early.push((after_ms, text));
        }
        final_text = decode(whisper, &samples, onset, samples.len() / FRAMES_PER_MS);
    }
    Backchannel {
        phrase,
        voice,
        rate_wpm,
        voiced_ms,
        final_text,
        cue_misfires,
        early,
    }
}

/// Whisper over the pre-roll and the first `after_ms` after the onset frame, as speech in
/// decodes a partial.
fn decode(whisper: &mut WhisperStt, samples: &[f32], onset: usize, after_ms: usize) -> String {
    let begin = onset.saturating_sub(SPEECH_PREROLL_MS as usize * FRAMES_PER_MS);
    let end = (onset + after_ms * FRAMES_PER_MS).min(samples.len());
    whisper.discard();
    whisper.push(&samples[begin..end]);
    whisper.partial().expect("decode").unwrap_or_default()
}

const CUE_PHRASES: [&str; 10] = [
    "Wait, why Redis first?",
    "Wait.",
    "Stop.",
    "Hold on, what about Kafka?",
    "Hang on a second.",
    "No, the other one.",
    "Actually, go back.",
    "Sorry, what was that?",
    "Why Redis?",
    "How does that work?",
];

/// Partial decode times after the onset, in ms.
const CUE_PARTIALS_MS: [u64; 8] = [160, 192, 224, 256, 288, 320, 384, 448];

/// How soon a partial hears the cue, and what each decode costs.
#[test]
#[ignore = "measurement: needs the Silero and Whisper models; see the module comment"]
fn spike_cue_words_heard_early() {
    let ollama = std::env::var("BOARDAI_SPIKE_OLLAMA")
        .ok()
        .map(|model| busy_ollama(&model));
    let load = if ollama.is_some() {
        "while Ollama generates"
    } else {
        "idle"
    };
    for device in [WhisperDevice::Gpu, WhisperDevice::Cpu] {
        let mut whisper =
            WhisperStt::new_on(&model(ModelId::WhisperBaseEn), device).expect("whisper loads");
        // The first decode pays for setting up the device.
        let _ = decode(&mut whisper, &vec![0.0; MIC_RATE_HZ as usize], 8_000, 200);
        println!("\ncue words · Whisper base.en on {device:?}, {load}");
        let mut heard_at = vec![0_usize; CUE_PARTIALS_MS.len()];
        let mut heard_with_candidates = vec![0_usize; CUE_PARTIALS_MS.len()];
        let mut decode_ms = Vec::new();
        let mut clips = 0;
        for voice in VOICES {
            for text in CUE_PHRASES {
                let samples = spoken(text, voice, None);
                let events = vad_events(&probabilities(&samples));
                let Some(onset) = events.iter().find_map(|(_, event)| match event {
                    VadEvent::Onset { onset_frame } => Some(*onset_frame as usize),
                    VadEvent::Start { .. } | VadEvent::End { .. } => None,
                }) else {
                    continue;
                };
                clips += 1;
                let (begin_ms, _) = rms_span_ms(&samples);
                let onset_lag = (onset / FRAMES_PER_MS) as i64 - begin_ms as i64;
                let mut line = Vec::new();
                for (index, after_ms) in CUE_PARTIALS_MS.iter().enumerate() {
                    let started = Instant::now();
                    let partial = decode(&mut whisper, &samples, onset, *after_ms as usize);
                    decode_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
                    let cue = has_interruption_cue(&partial);
                    heard_at[index] += usize::from(cue);
                    heard_with_candidates[index] +=
                        usize::from(cue || candidate_cue(&partial).is_some());
                    line.push(format!(
                        "{after_ms}:{}{partial:?}",
                        if cue { "+" } else { "-" }
                    ));
                }
                println!(
                    "  {:<22} onset {onset_lag:+} ms after the sound · {}",
                    voice_name(voice),
                    line.join(" ")
                );
            }
        }
        println!(
            "  cue heard by a partial at (after the onset): {}",
            rates(&heard_at, clips)
        );
        println!(
            "  with the candidate cues too: {}",
            rates(&heard_with_candidates, clips)
        );
        println!(
            "  decode p50 {:.0} / p90 {:.0} / max {:.0} ms over {} decodes",
            percentile(&decode_ms, 50),
            percentile(&decode_ms, 90),
            percentile(&decode_ms, 100),
            decode_ms.len()
        );
    }
    if let Some(mut ollama) = ollama {
        let _ = ollama.kill();
    }
}

/// `heard` per partial time, as "256 ms 29/50".
fn rates(heard: &[usize], clips: usize) -> String {
    let rates: Vec<String> = CUE_PARTIALS_MS
        .iter()
        .zip(heard)
        .map(|(ms, heard)| format!("{ms} ms {heard}/{clips}"))
        .collect();
    rates.join(", ")
}

/// Keeps Ollama generating a long answer in a loop until killed, so decodes share the GPU.
fn busy_ollama(model: &str) -> Child {
    let body = format!(
        r#"{{"model":"{model}","prompt":"Write a very long essay about distributed systems.","stream":false,"options":{{"num_predict":4000}}}}"#
    );
    let script = format!(
        "while true; do curl -s http://localhost:11434/api/generate -d '{body}' > /dev/null; done"
    );
    let child = Command::new("sh")
        .args(["-c", &script])
        .stdout(Stdio::null())
        .spawn()
        .expect("curl loop");
    // Let the first generation start.
    std::thread::sleep(std::time::Duration::from_secs(2));
    child
}

/// Sentences with a pause in the middle (`say`'s `[[slnc ms]]`): the detector ends the first
/// part, and its final is what the end-of-turn rule judges.
const PAUSED: [&str; 8] = [
    "So the leader [[slnc 900]] sends a heartbeat to every follower.",
    "And then the request [[slnc 900]] goes to the load balancer.",
    "I think the problem is [[slnc 900]] the cache.",
    "What happens when [[slnc 900]] two pods write at once?",
    "Can you show me [[slnc 900]] the handler?",
    "The client sends a request, and [[slnc 900]] the server answers.",
    "Hmm, so the leader [[slnc 900]] sends a heartbeat.",
    "Walk me through [[slnc 900]] how the cache gets invalidated.",
];

/// What Whisper writes for the first part of a sentence cut by a pause, and how the core
/// would judge it: a fragment it calls finished would end the turn early.
#[test]
#[ignore = "measurement: needs the Silero and Whisper models; see the module comment"]
fn spike_mid_sentence_pauses() {
    let mut whisper = WhisperStt::new(&model(ModelId::WhisperBaseEn)).expect("whisper loads");
    let mut counts = [0_usize; 3];
    println!("\nmid-sentence pauses · the first part's final, as speech in decodes it");
    for voice in VOICES {
        for text in PAUSED {
            let samples = spoken(text, voice, None);
            let events = vad_events(&probabilities(&samples));
            let mut finals = Vec::new();
            let mut begin = None;
            for (index, event) in &events {
                match event {
                    VadEvent::Start { onset_frame } => begin = Some(*onset_frame as usize),
                    VadEvent::End { .. } => {
                        let Some(onset) = begin.take() else { continue };
                        let end_ms = (index + 1) * VAD_WINDOW_FRAMES / FRAMES_PER_MS;
                        let after_ms = end_ms - onset / FRAMES_PER_MS;
                        finals.push(decode(&mut whisper, &samples, onset, after_ms));
                    }
                    VadEvent::Onset { .. } => {}
                }
            }
            let Some(first) = finals.first() else {
                continue;
            };
            let judged = completeness(first);
            counts[match judged {
                Completeness::Finished => 0,
                Completeness::Unclear => 1,
                Completeness::Unfinished => 2,
            }] += usize::from(finals.len() > 1);
            println!("  {:<22} {judged:?} {finals:?}", voice_name(voice));
        }
    }
    println!(
        "  first parts of split sentences: {} finished (the turn would end early), {} unclear, {} unfinished",
        counts[0], counts[1], counts[2]
    );
}

/// Sentences for the speculative end of turn: the natural ones (commas included), then pauses
/// shorter than the 256 ms hangover (a hesitation, a comma said slowly), then the 900 ms
/// pauses above, which already end the detector's segment.
const SPECULATIVE: [(&str, &str); 10] = [
    ("natural", SENTENCES[0]),
    ("natural", SENTENCES[3]),
    ("natural", SENTENCES[4]),
    ("natural", SENTENCES[5]),
    (
        "natural",
        "Okay, so the cache sits in front, and the database is behind it.",
    ),
    (
        "150 ms",
        "So the leader [[slnc 150]] sends a heartbeat to every follower.",
    ),
    (
        "200 ms",
        "What happens when [[slnc 200]] two pods write at once?",
    ),
    ("200 ms", "Can you show me [[slnc 200]] the handler?"),
    ("250 ms", "I think the problem is [[slnc 250]] the cache."),
    (
        "250 ms",
        "Walk me through [[slnc 250]] how the cache gets invalidated.",
    ),
];

/// What one early final would have done as a speculative request.
#[derive(Default)]
struct Speculation {
    /// Early finals decoded (after `EARLY_FINAL_AFTER_QUIET_MS` of quiet inside speech).
    early: usize,
    /// Of those, sounding finished: a speculative request would have started.
    started: usize,
    /// Of the started, speech resumed before `VoiceEnd`: the request is cancelled.
    cancelled: usize,
    /// Of the started, `VoiceEnd` followed: how much sooner the request went, in ms.
    saved_ms: Vec<f64>,
}

/// The speculative end of turn (T-016's proposal): start the talker on an early final that
/// sounds finished, instead of at `VoiceEnd`, and cancel it if speech resumes. Counts the
/// requests that would start, be cancelled, and the time won when they stand.
#[test]
#[ignore = "measurement: needs the Silero and Whisper models; see the module comment"]
fn spike_speculative_end_of_turn() {
    let mut whisper = WhisperStt::new(&model(ModelId::WhisperBaseEn)).expect("whisper loads");
    let _ = decode(&mut whisper, &vec![0.0; MIC_RATE_HZ as usize], 8_000, 200);
    let mut by_kind: Vec<(&str, Speculation)> = Vec::new();
    let mut decode_ms = Vec::new();
    println!("\nspeculative end of turn · early finals and what a request on them would do");
    for voice in VOICES {
        for (kind, text) in SPECULATIVE {
            let samples = spoken(text, voice, None);
            let index = by_kind
                .iter()
                .position(|(name, _)| *name == kind)
                .unwrap_or_else(|| {
                    by_kind.push((kind, Speculation::default()));
                    by_kind.len() - 1
                });
            let line = speculate(
                &mut whisper,
                &samples,
                &mut by_kind[index].1,
                &mut decode_ms,
            );
            println!("  {:<22} {kind:<8} {}", voice_name(voice), line.join(" · "));
        }
    }
    for (kind, tally) in &by_kind {
        println!(
            "  {kind:<8}: {} early finals, {} sound finished (a request starts), {} of those \
             cancelled (speech resumed); time won when it stands p50 {:.0} / min {:.0} ms",
            tally.early,
            tally.started,
            tally.cancelled,
            percentile(&tally.saved_ms, 50),
            percentile(&tally.saved_ms, 0),
        );
    }
    println!(
        "  early-final decode p50 {:.0} / p90 {:.0} ms",
        percentile(&decode_ms, 50),
        percentile(&decode_ms, 90)
    );
}

/// Runs one clip through the detector as speech in does, decoding the early final after
/// `EARLY_FINAL_AFTER_QUIET_MS` of quiet, and tallies what a request started on it would do.
fn speculate(
    whisper: &mut WhisperStt,
    samples: &[f32],
    tally: &mut Speculation,
    decode_ms: &mut Vec<f64>,
) -> Vec<String> {
    use boardai_runtime::speech_in::vad::EARLY_FINAL_AFTER_QUIET_MS;
    let mut machine = VadMachine::default();
    let mut onset = None;
    let mut pending: Option<(usize, String, f64)> = None;
    let mut line = Vec::new();
    for (window, p) in probabilities(samples).iter().enumerate() {
        let event = machine.step(*p);
        if let Some(VadEvent::Start { onset_frame }) = event {
            onset = Some(onset_frame as usize);
        }
        if matches!(event, Some(VadEvent::End { .. }))
            && let Some((at, early, ms)) = pending.take()
        {
            let finished = completeness(&early) == Completeness::Finished;
            if finished {
                tally
                    .saved_ms
                    .push((window - at) as f64 * WINDOW_MS as f64 - ms);
            }
            line.push(format!("end{}{early:?}", if finished { "+" } else { "-" }));
        }
        if !machine.in_speech() {
            continue;
        }
        let quiet = machine.quiet_ms();
        if quiet >= EARLY_FINAL_AFTER_QUIET_MS && pending.is_none() {
            let Some(onset) = onset else { continue };
            let after_ms = machine.next_frame() as usize / FRAMES_PER_MS - onset / FRAMES_PER_MS;
            let started = Instant::now();
            let early = decode(whisper, samples, onset, after_ms);
            let ms = started.elapsed().as_secs_f64() * 1_000.0;
            decode_ms.push(ms);
            tally.early += 1;
            tally.started += usize::from(completeness(&early) == Completeness::Finished);
            pending = Some((window, early, ms));
        } else if quiet == 0
            && let Some((_, early, _)) = pending.take()
        {
            let finished = completeness(&early) == Completeness::Finished;
            tally.cancelled += usize::from(finished);
            line.push(format!(
                "resumed{}{early:?}",
                if finished { "+" } else { "-" }
            ));
        }
    }
    line
}
