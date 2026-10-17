//! The speech-out actor with a fake engine and a simulated device: the real voice ring and
//! renderer, driven by a task that renders 10 ms per cycle on paused tokio time. No audio
//! hardware.
#![allow(clippy::expect_used, reason = "tests fail loudly")]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::effect::Gain;
use boardai_core::ids::ClauseId;
use boardai_core::input::{Input, PlaybackPosition, TimingSource, WordTiming};
use boardai_runtime::audio::{PlaybackRenderer, VOICE_RING_FRAMES, voice_ring};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::speech_out::{
    ClauseText, SpeechOut, SpeechOutChannels, SpeechOutCommand, SpeechOutConfig, TextToSpeech,
    TtsChunk, TtsError, TtsStream, VoiceId,
};
use futures_util::stream;
use tokio::task::JoinHandle;

const FRAMES_PER_MS: u64 = 24;
const CYCLE_FRAMES: usize = 240;

/// Synthesizes `ms_per_clause` of a constant 0.5 per clause, with exact timings when asked.
struct FakeVoice {
    ms_per_clause: u32,
    exact: bool,
    failing: BTreeSet<u32>,
    calls: Arc<AtomicUsize>,
}

impl FakeVoice {
    fn new(ms_per_clause: u32) -> Self {
        Self {
            ms_per_clause,
            exact: true,
            failing: BTreeSet::new(),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl TextToSpeech for FakeVoice {
    fn synthesize(&self, clause: &ClauseText, _voice: &VoiceId, _cancel: CancelToken) -> TtsStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.failing.contains(&clause.id.0) {
            let error = TtsError::Network {
                detail: "offline".into(),
            };
            return Box::pin(stream::iter(vec![Err(error)]));
        }
        let frames = (u64::from(self.ms_per_clause) * FRAMES_PER_MS) as usize;
        let mut chunks = vec![
            Ok(TtsChunk::Audio(vec![0.5; frames / 2])),
            Ok(TtsChunk::Audio(vec![0.5; frames - frames / 2])),
        ];
        if self.exact {
            chunks.push(Ok(TtsChunk::Words(vec![WordTiming {
                byte_offset: 0,
                start_ms: 5,
            }])));
        }
        Box::pin(stream::iter(chunks))
    }
}

struct Harness {
    speech: SpeechOut,
    channels: SpeechOutChannels,
    device: Option<JoinHandle<()>>,
    rendered: Arc<Mutex<Vec<f32>>>,
    /// While set, the device renders nothing: the audio unit is being rebuilt.
    paused: Arc<AtomicBool>,
}

impl Harness {
    fn start(
        engine: FakeVoice,
        latency_frames: u64,
        fallback: Option<FakeVoice>,
    ) -> (Self, PlaybackRenderer) {
        let (output, renderer) = voice_ring(VOICE_RING_FRAMES, latency_frames);
        let config = SpeechOutConfig {
            voice: VoiceId::default(),
            fallback: fallback
                .map(|voice| (Arc::new(voice) as Arc<dyn TextToSpeech>, VoiceId::default())),
        };
        let (speech, channels) = SpeechOut::spawn(Arc::new(engine), config, output);
        let harness = Self {
            speech,
            channels,
            device: None,
            rendered: Arc::new(Mutex::new(Vec::new())),
            paused: Arc::new(AtomicBool::new(false)),
        };
        (harness, renderer)
    }

    /// Renders one 10 ms cycle every 10 ms, recording what it played.
    fn run_device(&mut self, mut renderer: PlaybackRenderer) {
        let rendered = Arc::clone(&self.rendered);
        let paused = Arc::clone(&self.paused);
        self.device = Some(tokio::spawn(async move {
            let mut buffer = vec![0.0; CYCLE_FRAMES];
            loop {
                if paused.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                renderer.begin_cycle();
                renderer.render(&mut buffer);
                renderer.end_cycle();
                rendered.lock().expect("lock").extend_from_slice(&buffer);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }));
    }

    async fn send(&self, command: SpeechOutCommand) {
        self.speech
            .sender()
            .send(command)
            .await
            .expect("actor running");
    }

    async fn say(&self, clause: u32) {
        self.send(SpeechOutCommand::Synthesize {
            clause: ClauseId(clause),
            text: format!("clause {clause} speaks"),
        })
        .await;
    }

    async fn next_event(&mut self) -> Input {
        tokio::time::timeout(Duration::from_secs(60), self.channels.events.recv())
            .await
            .expect("an event in time")
            .expect("actor running")
    }

    /// Positions published until `until` resolves, sampled at every change.
    fn record_positions(&self) -> (Arc<Mutex<Vec<PlaybackPosition>>>, JoinHandle<()>) {
        let positions = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&positions);
        let mut playback = self.channels.playback.clone();
        let task = tokio::spawn(async move {
            while playback.changed().await.is_ok() {
                if let Some(position) = *playback.borrow_and_update() {
                    sink.lock().expect("lock").push(position);
                }
            }
        });
        (positions, task)
    }

    async fn finish(self) {
        if let Some(device) = self.device {
            device.abort();
        }
        self.speech.shutdown().await;
    }
}

fn ready_clause(input: &Input) -> Option<(u32, TimingSource, u32)> {
    let Input::ClauseReady { clause, timings } = input else {
        return None;
    };
    Some((clause.0, timings.source, timings.duration_ms))
}

#[tokio::test(start_paused = true)]
async fn clauses_play_in_order_and_drain() {
    let (mut harness, renderer) = Harness::start(FakeVoice::new(400), 0, None);
    harness.run_device(renderer);
    let (positions, recorder) = harness.record_positions();
    harness.say(1).await;
    harness.say(2).await;
    let first = harness.next_event().await;
    assert_eq!(ready_clause(&first), Some((1, TimingSource::Exact, 400)));
    let second = harness.next_event().await;
    assert_eq!(ready_clause(&second), Some((2, TimingSource::Exact, 400)));
    assert_eq!(harness.next_event().await, Input::PlaybackDrained);
    recorder.abort();
    let positions = positions.lock().expect("lock").clone();
    assert!(positions.iter().any(|p| p.clause == ClauseId(1)));
    assert!(
        positions
            .iter()
            .any(|p| p.clause == ClauseId(2) && p.offset_ms >= 380)
    );
    let keys: Vec<(u32, u32)> = positions
        .iter()
        .map(|p| (p.clause.0, p.offset_ms))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "positions only move forward");
    // Everything synthesized was played, at full level.
    let rendered = harness.rendered.lock().expect("lock").clone();
    let voiced = rendered.iter().filter(|sample| **sample > 0.49).count();
    assert_eq!(voiced, 800 * FRAMES_PER_MS as usize);
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn positions_lag_by_the_output_latency() {
    let latency_ms = 100;
    let (mut harness, renderer) =
        Harness::start(FakeVoice::new(1_000), latency_ms * FRAMES_PER_MS, None);
    harness.say(1).await;
    assert!(ready_clause(&harness.next_event().await).is_some());
    harness.run_device(renderer);
    tokio::time::sleep(Duration::from_millis(505)).await;
    let position = (*harness.channels.playback.borrow()).expect("playing");
    // About 500 ms were sent to the device; 100 ms of it is still on its way to the speaker.
    let offset = i64::from(position.offset_ms);
    assert!((offset - 400).abs() <= 30, "{offset}");
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn a_device_change_pauses_playback_and_positions_never_go_back() {
    let (mut harness, renderer) = Harness::start(FakeVoice::new(2_000), 30 * FRAMES_PER_MS, None);
    let monitor = renderer.monitor();
    harness.say(1).await;
    assert!(ready_clause(&harness.next_event().await).is_some());
    let (positions, recorder) = harness.record_positions();
    harness.run_device(renderer);
    tokio::time::sleep(Duration::from_millis(500)).await;
    // The unit is rebuilt on a slower device (Bluetooth): a gap, then a longer latency.
    harness.paused.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = (*harness.channels.playback.borrow()).expect("playing");
    monitor.set_output_latency_frames(200 * FRAMES_PER_MS);
    harness.paused.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after = (*harness.channels.playback.borrow()).expect("playing");
    assert!(
        after.offset_ms >= before.offset_ms,
        "{before:?} then {after:?}"
    );
    assert_eq!(harness.next_event().await, Input::PlaybackDrained);
    recorder.abort();
    let offsets: Vec<u32> = positions
        .lock()
        .expect("lock")
        .iter()
        .map(|p| p.offset_ms)
        .collect();
    assert!(
        offsets.windows(2).all(|pair| pair[0] <= pair[1]),
        "{offsets:?}"
    );
    // Nothing in the ring was lost to the gap.
    let rendered = harness.rendered.lock().expect("lock").clone();
    let voiced = rendered.iter().filter(|sample| **sample > 0.49).count();
    assert_eq!(voiced, 2_000 * FRAMES_PER_MS as usize);
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn stop_voice_reports_what_was_heard_and_drops_the_rest() {
    let (mut harness, renderer) = Harness::start(FakeVoice::new(2_000), 0, None);
    harness.say(1).await;
    assert!(ready_clause(&harness.next_event().await).is_some());
    harness.run_device(renderer);
    tokio::time::sleep(Duration::from_millis(500)).await;
    harness.send(SpeechOutCommand::StopVoice).await;
    let Input::VoiceStopped { last: Some(last) } = harness.next_event().await else {
        panic!("a position")
    };
    assert_eq!(last.clause, ClauseId(1));
    // Half a second played, then the 30 ms fade (plus at most a cycle of slack).
    assert!((500..=560).contains(&last.offset_ms), "{}", last.offset_ms);
    assert_eq!(*harness.channels.playback.borrow(), None);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rendered = harness.rendered.lock().expect("lock").clone();
    let heard = rendered.iter().filter(|sample| **sample > 0.0).count() as u64;
    assert!(
        heard <= u64::from(last.offset_ms + 10) * FRAMES_PER_MS,
        "{heard}"
    );
    // The next clause starts from its beginning.
    harness.say(2).await;
    assert_eq!(
        ready_clause(&harness.next_event().await).map(|r| r.0),
        Some(2)
    );
    assert_eq!(harness.next_event().await, Input::PlaybackDrained);
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn stop_voice_while_silent_answers_none() {
    let (mut harness, renderer) = Harness::start(FakeVoice::new(100), 0, None);
    harness.run_device(renderer);
    harness.send(SpeechOutCommand::StopVoice).await;
    assert_eq!(
        harness.next_event().await,
        Input::VoiceStopped { last: None }
    );
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn a_stalled_device_still_gets_an_answer() {
    let (mut harness, _renderer) = Harness::start(FakeVoice::new(100), 0, None);
    harness.send(SpeechOutCommand::StopVoice).await;
    let started = tokio::time::Instant::now();
    assert_eq!(
        harness.next_event().await,
        Input::VoiceStopped { last: None }
    );
    assert!(started.elapsed() >= Duration::from_millis(250));
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn ducking_lowers_the_voice() {
    let (mut harness, renderer) = Harness::start(FakeVoice::new(1_000), 0, None);
    harness.say(1).await;
    assert!(ready_clause(&harness.next_event().await).is_some());
    harness
        .send(SpeechOutCommand::Duck { gain: Gain::DUCKED })
        .await;
    harness.run_device(renderer);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let rendered = harness.rendered.lock().expect("lock").clone();
    let tail = &rendered[rendered.len() - CYCLE_FRAMES..];
    assert!(
        tail.iter().all(|sample| (*sample - 0.125).abs() < 1e-3),
        "{:?}",
        &tail[..4]
    );
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn synthesis_stays_within_the_lookahead() {
    let voice = FakeVoice::new(5_000);
    let calls = Arc::clone(&voice.calls);
    let (mut harness, _renderer) = Harness::start(voice, 0, None);
    for clause in 1..=8 {
        harness.say(clause).await;
    }
    for _ in 0..3 {
        assert!(ready_clause(&harness.next_event().await).is_some());
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    // Nothing plays, so after 15 s of audio (three 5 s clauses) synthesis waits.
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_clause_falls_back_to_the_other_voice() {
    let mut main = FakeVoice::new(200);
    main.failing.insert(1);
    let fallback = FakeVoice::new(300);
    let (mut harness, renderer) = Harness::start(main, 0, Some(fallback));
    harness.run_device(renderer);
    harness.say(1).await;
    assert_eq!(
        ready_clause(&harness.next_event().await),
        Some((1, TimingSource::Exact, 300))
    );
    assert_eq!(harness.next_event().await, Input::PlaybackDrained);
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn a_clause_that_fails_everywhere_is_reported_and_skipped() {
    let mut main = FakeVoice::new(200);
    main.failing.insert(1);
    let (mut harness, renderer) = Harness::start(main, 0, None);
    harness.run_device(renderer);
    harness.say(1).await;
    harness.say(2).await;
    // The core is told, so it skips the clause and its ops instead of waiting forever (D-059).
    assert_eq!(
        harness.next_event().await,
        Input::SynthesisFailed {
            clause: ClauseId(1)
        }
    );
    assert_eq!(
        ready_clause(&harness.next_event().await).map(|r| r.0),
        Some(2)
    );
    assert_eq!(harness.next_event().await, Input::PlaybackDrained);
    harness.finish().await;
}

#[tokio::test(start_paused = true)]
async fn an_engine_without_timings_gets_estimates() {
    let mut voice = FakeVoice::new(600);
    voice.exact = false;
    let (mut harness, _renderer) = Harness::start(voice, 0, None);
    harness.say(4).await;
    let Input::ClauseReady { timings, .. } = harness.next_event().await else {
        panic!("clause ready")
    };
    assert_eq!(timings.source, TimingSource::Estimated);
    let starts: Vec<u32> = timings.words.iter().map(|word| word.byte_offset).collect();
    assert_eq!(starts, vec![0, 7, 9]);
    harness.finish().await;
}
