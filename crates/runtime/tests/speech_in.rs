//! The speech-in threads with a fake detector (speech is any loud window) and a fake
//! speech-to-text, fed through the real mic ring. No models, no microphone.
#![allow(clippy::expect_used, reason = "tests fail loudly")]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use boardai_core::input::Input;
use boardai_runtime::audio::{MIC_RATE_HZ, MIC_RING_FRAMES, MicWriter, mic_ring};
use boardai_runtime::speech_in::{
    SpeechIn, SpeechToText, SttError, VAD_WINDOW_FRAMES, VadError, VoiceDetector,
};
use tokio::sync::mpsc;

const FRAMES_PER_MS: usize = MIC_RATE_HZ as usize / 1_000;

struct LoudnessDetector {
    fail: bool,
}

impl VoiceDetector for LoudnessDetector {
    fn probability(&mut self, window: &[f32; VAD_WINDOW_FRAMES]) -> Result<f32, VadError> {
        if self.fail {
            return Err(VadError::Inference {
                detail: "planted".into(),
            });
        }
        let loud = window.iter().map(|sample| sample.abs()).sum::<f32>() / window.len() as f32;
        Ok(if loud > 0.1 { 0.95 } else { 0.02 })
    }

    fn reset(&mut self) {}
}

/// Says how many frames it has heard, so tests can check what reached it.
#[derive(Clone, Default)]
struct CountingStt {
    frames: Arc<Mutex<usize>>,
    vocabulary: Arc<Mutex<Vec<String>>>,
}

impl SpeechToText for CountingStt {
    fn push(&mut self, pcm: &[f32]) {
        *self.frames.lock().expect("lock") += pcm.len();
    }

    fn partial(&mut self) -> Result<Option<String>, SttError> {
        Ok(Some(format!(
            "heard {}",
            *self.frames.lock().expect("lock")
        )))
    }

    fn finish(&mut self) -> Result<String, SttError> {
        let mut frames = self.frames.lock().expect("lock");
        let text = format!("final {}", *frames);
        *frames = 0;
        Ok(text)
    }

    fn discard(&mut self) {
        *self.frames.lock().expect("lock") = 0;
    }

    fn set_vocabulary(&mut self, terms: &[String]) {
        self.vocabulary
            .lock()
            .expect("lock")
            .extend_from_slice(terms);
    }
}

fn audio(ms: usize, level: f32) -> Vec<f32> {
    vec![level; ms * FRAMES_PER_MS]
}

fn start(fail: bool) -> (SpeechIn, mpsc::Receiver<Input>, MicWriter, CountingStt) {
    let (writer, reader) = mic_ring(MIC_RING_FRAMES);
    let stt = CountingStt::default();
    let (speech, events) = SpeechIn::spawn(
        reader,
        Box::new(LoudnessDetector { fail }),
        Box::new(stt.clone()),
    )
    .expect("threads start");
    (speech, events, writer, stt)
}

/// Writes `samples` at real-time pace, in 32 ms windows.
fn speak_in_real_time(writer: &mut MicWriter, samples: &[f32]) {
    let started = Instant::now();
    for (index, window) in samples.chunks(VAD_WINDOW_FRAMES).enumerate() {
        writer.write(window);
        let due = Duration::from_millis(32 * (index as u64 + 1));
        std::thread::sleep(due.saturating_sub(started.elapsed()));
    }
}

fn next(events: &mut mpsc::Receiver<Input>) -> Input {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match events.try_recv() {
            Ok(input) => return input,
            Err(mpsc::error::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("no event: {error:?}"),
        }
    }
}

/// The onset comes first, at the first loud window; `VoiceStart` once the speech has lasted.
fn expect_start(events: &mut mpsc::Receiver<Input>) {
    assert_eq!(next(events), Input::VoiceOnset);
    assert_eq!(next(events), Input::VoiceStart);
}

#[test]
fn a_click_is_an_onset_and_nothing_more() {
    let (speech, mut events, mut writer, stt) = start(false);
    writer.write(&audio(384, 0.0));
    writer.write(&audio(64, 0.5));
    writer.write(&audio(600, 0.0));
    assert_eq!(next(&mut events), Input::VoiceOnset);
    std::thread::sleep(Duration::from_millis(200));
    assert!(events.try_recv().is_err(), "no start, end or transcript");
    assert_eq!(
        *stt.frames.lock().expect("lock"),
        0,
        "nothing reached Whisper"
    );
    speech.shutdown();
}

#[test]
fn an_utterance_gives_start_end_and_a_final_with_its_preroll() {
    let (speech, mut events, mut writer, _stt) = start(false);
    // Window-aligned: 12 windows of silence, 32 of speech.
    writer.write(&audio(384, 0.0));
    writer.write(&audio(1_024, 0.5));
    // 1.8 s in all: within the 2 s ring even if the VAD thread has not read any of it yet.
    writer.write(&audio(400, 0.0));
    expect_start(&mut events);
    assert_eq!(next(&mut events), Input::VoiceEnd { voiced_ms: 1_024 });
    let Input::Transcript { text, is_final } = next(&mut events) else {
        panic!("a transcript")
    };
    assert!(is_final);
    // Decoded early, after 96 ms of quiet: 300 ms of pre-roll, the speech, three quiet windows.
    let expected = (300 + 1_024 + 96) * FRAMES_PER_MS;
    assert_eq!(text, format!("heard {expected}"));
    speech.shutdown();
}

#[test]
fn speaking_again_after_a_pause_discards_the_early_final() {
    let (speech, mut events, mut writer, _stt) = start(false);
    writer.write(&audio(384, 0.0));
    writer.write(&audio(512, 0.5));
    // 160 ms of quiet: long enough to decode early, too short to end the utterance.
    writer.write(&audio(160, 0.0));
    writer.write(&audio(512, 0.5));
    writer.write(&audio(320, 0.0));
    expect_start(&mut events);
    assert_eq!(next(&mut events), Input::VoiceEnd { voiced_ms: 1_184 });
    let Input::Transcript { text, is_final } = next(&mut events) else {
        panic!("a transcript")
    };
    assert!(is_final);
    // The first early final (after 300 + 512 + 96 ms) was thrown away when speech resumed; the
    // one taken in the last pause covers both phrases.
    let expected = (300 + 512 + 160 + 512 + 96) * FRAMES_PER_MS;
    assert_eq!(text, format!("heard {expected}"));
    speech.shutdown();
}

#[test]
fn partials_arrive_while_you_speak() {
    let (speech, mut events, mut writer, _stt) = start(false);
    speak_in_real_time(&mut writer, &audio(1_800, 0.5));
    speak_in_real_time(&mut writer, &audio(320, 0.0));
    expect_start(&mut events);
    let mut partials = Vec::new();
    loop {
        let input = next(&mut events);
        if matches!(input, Input::VoiceEnd { .. }) {
            break;
        }
        let Input::Transcript {
            text,
            is_final: false,
        } = input
        else {
            panic!("unexpected {input:?}")
        };
        partials.push(text);
    }
    // 1.8 s of speech at one partial per 500 ms: three, each over more audio than the last.
    assert!((2..=4).contains(&partials.len()), "{partials:?}");
    let counts: Vec<usize> = partials
        .iter()
        .filter_map(|text| text.strip_prefix("heard ")?.parse().ok())
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0] < pair[1]),
        "{counts:?}"
    );
    assert!(matches!(
        next(&mut events),
        Input::Transcript { is_final: true, .. }
    ));
    speech.shutdown();
}

#[test]
fn muting_drops_the_utterance_unheard() {
    let (speech, mut events, mut writer, stt) = start(false);
    writer.write(&audio(400, 0.5));
    expect_start(&mut events);
    speech.set_muted(true);
    std::thread::sleep(Duration::from_millis(50));
    writer.write(&audio(1_000, 0.0));
    std::thread::sleep(Duration::from_millis(100));
    assert!(events.try_recv().is_err(), "nothing after the mute");
    assert_eq!(*stt.frames.lock().expect("lock"), 0);
    speech.set_muted(false);
    writer.write(&audio(400, 0.5));
    writer.write(&audio(400, 0.0));
    expect_start(&mut events);
    assert!(matches!(next(&mut events), Input::VoiceEnd { .. }));
    speech.shutdown();
}

#[test]
fn a_failing_detector_counts_as_silence() {
    let (speech, mut events, mut writer, _stt) = start(true);
    writer.write(&audio(500, 0.5));
    std::thread::sleep(Duration::from_millis(200));
    assert!(events.try_recv().is_err());
    assert!(speech.counters().detector_errors.load(Ordering::Relaxed) > 0);
    speech.shutdown();
}

#[test]
fn vocabulary_reaches_the_engine() {
    let (speech, _events, _writer, stt) = start(false);
    speech.set_vocabulary(vec!["getUserById".into(), "Redis".into()]);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        *stt.vocabulary.lock().expect("lock"),
        vec!["getUserById", "Redis"]
    );
    speech.shutdown();
}
