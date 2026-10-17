//! The session with the real speech-out actor behind [`LiveVoice`] (a fake engine and a
//! simulated device on paused time), and with real speech-in threads behind [`LiveListener`]
//! (a fake detector and recognizer fed through the real mic ring). No models, no hardware.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boardai_core::effect::{Gain, Milestone};
use boardai_core::input::{Input, TimingSource, WordTiming};
use boardai_core::protocol::{MicAction, UiCommand, UiEvent};
use boardai_core::{CoreConfig, MicMode, RepoReads, RequestStart};
use futures_util::stream;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{DEMO, Harness, Recorder, ScriptedTalker, close, idle_after_speaking, run_until};
use crate::audio::{
    MIC_RATE_HZ, MIC_RING_FRAMES, MicWriter, PlaybackRenderer, VOICE_RATE_HZ, VOICE_RING_FRAMES,
    VoiceGain, mic_ring, voice_ring,
};
use crate::bridge::UiSink;
use crate::cancel::CancelToken;
use crate::clock::TokioClock;
use crate::limits::{SESSION_IDLE_TIMEOUT_MS, VOICE_FADE_MS};
use crate::llm::LlmProvider;
use crate::session::{
    Listening, LiveListener, LiveVoice, ProblemLink, SessionChannels, SessionParts, StoreSink,
    VirtualVoice, VirtualVoiceConfig, say_typed, spawn,
};
use crate::speech_in::{
    SpeechIn, SpeechToText, SttError, VAD_WINDOW_FRAMES, VadError, VoiceDetector,
};
use crate::speech_out::{ClauseText, SpeechOutConfig, TextToSpeech, TtsChunk, TtsStream, VoiceId};
use crate::tools::RepoTools;

/// Milliseconds of fake voice per byte of text: a steady 0.5 tone with exact word starts.
const TONE_MS_PER_BYTE: u32 = 40;
const CYCLE_MS: u64 = 10;
const CYCLE_FRAMES: usize = (VOICE_RATE_HZ as usize / 1_000) * CYCLE_MS as usize;

struct ToneVoice;

impl TextToSpeech for ToneVoice {
    fn synthesize(&self, clause: &ClauseText, _voice: &VoiceId, _cancel: CancelToken) -> TtsStream {
        let frames_per_ms = VOICE_RATE_HZ / 1_000;
        let length = u32::try_from(clause.text.len()).unwrap_or(u32::MAX);
        let frames = (length * TONE_MS_PER_BYTE * frames_per_ms) as usize;
        let mut words = Vec::new();
        let mut after_space = true;
        for (offset, ch) in clause.text.char_indices() {
            if after_space && !ch.is_whitespace() {
                let byte_offset = u32::try_from(offset).unwrap_or(u32::MAX);
                words.push(WordTiming {
                    byte_offset,
                    start_ms: byte_offset * TONE_MS_PER_BYTE,
                });
            }
            after_space = ch.is_whitespace();
        }
        Box::pin(stream::iter(vec![
            Ok(TtsChunk::Audio(vec![0.5; frames])),
            Ok(TtsChunk::Words(words)),
        ]))
    }
}

/// Plays the voice ring 10 ms at a time and records the loudest sample of each cycle.
fn run_device(mut renderer: PlaybackRenderer) -> (Arc<Mutex<Vec<f32>>>, JoinHandle<()>) {
    let levels = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&levels);
    let task = tokio::spawn(async move {
        let mut buffer = vec![0.0; CYCLE_FRAMES];
        loop {
            renderer.begin_cycle();
            renderer.render(&mut buffer);
            renderer.end_cycle();
            let loudest = buffer
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            if let Ok(mut levels) = sink.lock() {
                levels.push(loudest);
            }
            tokio::time::sleep(Duration::from_millis(CYCLE_MS)).await;
        }
    });
    (levels, task)
}

struct LiveHarness {
    harness: Harness,
    gain: VoiceGain,
    levels: Arc<Mutex<Vec<f32>>>,
    device: JoinHandle<()>,
    listening: watch::Receiver<Listening>,
    problems: ProblemLink,
}

fn parts(
    talker: &Arc<ScriptedTalker>,
    recorder: &Arc<Recorder>,
    speech: Box<dyn crate::session::SpeechOut>,
) -> SessionParts {
    SessionParts {
        core: CoreConfig {
            talker_name: "the model".to_owned(),
            mic: MicMode::OpenMic,
            session_context: None,
            suggestion: None,
            repo_reads: RepoReads::NotOffered,
            request_start: RequestStart::AtTurnEnd,
        },
        clock: TokioClock::start(),
        talker: Arc::clone(talker) as Arc<dyn LlmProvider>,
        tools: RepoTools::default(),
        harness: None,
        speech,
        store: Arc::clone(recorder) as Arc<dyn StoreSink>,
        ui: Arc::clone(recorder) as Arc<dyn UiSink>,
        sources: crate::session::SourceParts::in_memory(std::path::PathBuf::from("/")),
    }
}

fn start_live(talker: Arc<ScriptedTalker>) -> LiveHarness {
    let channels = SessionChannels::new();
    let (output, renderer) = voice_ring(VOICE_RING_FRAMES, 0);
    let gain = output.gain_control();
    let config = SpeechOutConfig {
        voice: VoiceId::default(),
        fallback: None,
    };
    let voice = LiveVoice::spawn(channels.speech_out(), Arc::new(ToneVoice), config, output);
    let (levels, device) = run_device(renderer);
    let recorder = Arc::new(Recorder::default());
    let parts = parts(&talker, &recorder, Box::new(voice));
    let listening = channels.listening();
    let problems = channels.problems();
    let harness = Harness {
        session: spawn(channels, parts),
        talker,
        recorder,
    };
    LiveHarness {
        harness,
        gain,
        levels,
        device,
        listening,
        problems,
    }
}

fn loud_cycles(levels: &Mutex<Vec<f32>>) -> usize {
    levels
        .lock()
        .map(|levels| levels.iter().filter(|level| **level > 0.4).count())
        .unwrap_or(0)
}

fn is_full(gain: &VoiceGain) -> bool {
    (gain.target().value() - Gain::FULL.value()).abs() < f32::EPSILON
}

fn is_ducked(gain: &VoiceGain) -> bool {
    (gain.target().value() - Gain::DUCKED.value()).abs() < f32::EPSILON
}

/// The steel thread on the real speech-out actor: the answer is synthesized, played through the
/// ring, its positions drive the strokes, and the drain ends the turn.
#[tokio::test(start_paused = true)]
async fn the_real_voice_plays_the_answer_and_its_clock_draws_the_board() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    let live = start_live(Arc::clone(&talker));
    let link = live.harness.session.speech_in();
    assert!(
        say_typed(&link, "How does a request reach our API?")
            .await
            .is_ok()
    );
    let recorder = Arc::clone(&live.harness.recorder);
    assert!(run_until(60_000, || idle_after_speaking(&recorder)).await);
    let played_ms = loud_cycles(&live.levels) as u64 * CYCLE_MS;
    live.device.abort();
    let (_, recorder, _) = close(live.harness).await;
    let events = recorder.events();
    let ops = events
        .iter()
        .filter(|event| matches!(event, UiEvent::OpStart { .. }))
        .count();
    assert_eq!(ops, 3, "the browser, the CDN and the edge between them");
    let inputs = recorder.inputs();
    let exact = inputs.iter().any(|input| {
        matches!(input, Input::ClauseReady { timings, .. } if timings.source == TimingSource::Exact)
    });
    assert!(exact, "the engine's word timings reached the core");
    assert!(inputs.contains(&Input::PlaybackDrained));
    assert!(recorder.marks().contains(&Milestone::FirstAudio));
    // Every clause's tone played at full level (the speakable text is about 60 bytes).
    assert!(played_ms >= 2_000, "{played_ms} ms of voice played");
}

/// An onset ducks at once, before any speech is confirmed; the interruption then stops the
/// voice, the heard prefix is committed, and the gain comes back for the next answer.
#[tokio::test(start_paused = true)]
async fn an_onset_ducks_at_once_and_the_interruption_stops_the_voice() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    talker.says("Because reads are cheap there.");
    let live = start_live(Arc::clone(&talker));
    let link = live.harness.session.speech_in();
    assert!(say_typed(&link, "Walk me through it.").await.is_ok());
    let levels = Arc::clone(&live.levels);
    assert!(run_until(30_000, || loud_cycles(&levels) >= 40).await);
    assert!(link.voice_onset().await.is_ok());
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(
        is_ducked(&live.gain),
        "ducked on the onset, before VoiceStart"
    );
    assert!(say_typed(&link, "wait, why Redis first?").await.is_ok());
    let recorder = Arc::clone(&live.harness.recorder);
    let stopped = || {
        recorder
            .inputs()
            .iter()
            .any(|input| matches!(input, Input::VoiceStopped { last: Some(_) }))
    };
    assert!(run_until(5_000, stopped).await);
    let at_stop = loud_cycles(&live.levels);
    tokio::time::sleep(Duration::from_millis(u64::from(VOICE_FADE_MS) + 100)).await;
    let after_fade = loud_cycles(&live.levels);
    assert_eq!(at_stop, after_fade, "no voice after the fade");
    assert!(is_full(&live.gain), "the gain is back for the next answer");
    assert!(run_until(30_000, || idle_after_speaking(&recorder)).await);
    live.device.abort();
    let (_, recorder, talker) = close(live.harness).await;
    let marks = recorder.marks();
    for mark in [
        Milestone::OnsetDuck,
        Milestone::BargeInDuck,
        Milestone::BargeInStop,
    ] {
        assert!(marks.contains(&mark), "{mark:?} in {marks:?}");
    }
    let requests = talker.requests();
    assert_eq!(requests.len(), 2, "the interruption was answered");
}

#[tokio::test(start_paused = true)]
async fn a_cough_ducks_and_the_gain_comes_back() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    let live = start_live(Arc::clone(&talker));
    let link = live.harness.session.speech_in();
    assert!(say_typed(&link, "Walk me through it.").await.is_ok());
    let levels = Arc::clone(&live.levels);
    assert!(run_until(30_000, || loud_cycles(&levels) >= 40).await);
    assert!(link.voice_onset().await.is_ok());
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(is_ducked(&live.gain));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(is_full(&live.gain), "restored when the hold ended");
    let recorder = Arc::clone(&live.harness.recorder);
    assert!(run_until(60_000, || idle_after_speaking(&recorder)).await);
    live.device.abort();
    let (_, recorder, _) = close(live.harness).await;
    assert!(
        !recorder
            .inputs()
            .iter()
            .any(|input| matches!(input, Input::VoiceStopped { .. })),
        "a cough never stops the voice"
    );
}

/// The session tells speech in when the mic is heard (mute) and which words were drawn.
#[tokio::test(start_paused = true)]
async fn listening_follows_mute_and_the_board() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    let channels = SessionChannels::new();
    let mut listening = channels.listening();
    let voice = VirtualVoice::spawn(channels.speech_out(), VirtualVoiceConfig::default());
    let recorder = Arc::new(Recorder::default());
    let parts = parts(&talker, &recorder, Box::new(voice));
    let session = spawn(channels, parts);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), listening.wait_for(|now| now.open))
            .await
            .is_ok(),
        "open once the session starts"
    );
    assert!(
        say_typed(&session.speech_in(), "Walk me through it.")
            .await
            .is_ok()
    );
    let terms = tokio::time::timeout(
        Duration::from_secs(60),
        listening.wait_for(|now| now.recent_terms.len() >= 2),
    )
    .await;
    let Ok(Ok(terms)) = terms.map(|seen| seen.map(|now| now.recent_terms.clone())) else {
        panic!("board labels became terms");
    };
    assert_eq!(terms.front().map(String::as_str), Some("CDN"));
    let mute = UiCommand::Mic {
        action: MicAction::Mute,
    };
    assert!(session.command(mute).is_ok());
    let muted = tokio::time::timeout(Duration::from_secs(1), listening.wait_for(|now| !now.open));
    assert!(matches!(muted.await, Ok(Ok(_))), "closed on mute");
    let _ = session.close().await;
}

// ---------------------------------------------------------------------------------------------
// Speech in, through the real mic ring and threads (real time: the threads sleep).

struct LoudnessDetector;

impl VoiceDetector for LoudnessDetector {
    fn probability(&mut self, window: &[f32; VAD_WINDOW_FRAMES]) -> Result<f32, VadError> {
        let level = window.iter().map(|sample| sample.abs()).sum::<f32>() / window.len() as f32;
        Ok(if level > 0.1 { 0.95 } else { 0.02 })
    }

    fn reset(&mut self) {}
}

/// Says "Walk me through it." for any audio, and keeps the vocabulary it is given.
#[derive(Clone, Default)]
struct FixedStt {
    vocabulary: Arc<Mutex<Vec<String>>>,
    vocabulary_updates: Arc<AtomicU32>,
}

impl SpeechToText for FixedStt {
    fn push(&mut self, _pcm: &[f32]) {}

    // The early final is a partial decode, so partials say the whole sentence too.
    fn partial(&mut self) -> Result<Option<String>, SttError> {
        Ok(Some("Walk me through it.".to_owned()))
    }

    fn finish(&mut self) -> Result<String, SttError> {
        Ok("Walk me through it.".to_owned())
    }

    fn discard(&mut self) {}

    fn set_vocabulary(&mut self, terms: &[String]) {
        self.vocabulary_updates.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut vocabulary) = self.vocabulary.lock() {
            vocabulary.clone_from(&terms.to_vec());
        }
    }
}

fn speak(writer: &mut MicWriter, ms: usize, level: f32) {
    let frames = ms * MIC_RATE_HZ as usize / 1_000;
    writer.write(&vec![level; frames]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spoken_words_reach_the_core_and_the_board_reaches_whisper() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    let channels = SessionChannels::new();
    let (mut writer, reader) = mic_ring(MIC_RING_FRAMES);
    let stt = FixedStt::default();
    let (speech_in, inputs) =
        SpeechIn::spawn(reader, Box::new(LoudnessDetector), Box::new(stt.clone()))
            .unwrap_or_else(|error| panic!("speech in starts: {error}"));
    let repo_terms = vec!["SessionChannels".to_owned()];
    let listener = LiveListener::spawn(
        speech_in,
        inputs,
        channels.speech_in(),
        channels.listening(),
        repo_terms,
    );
    let voice = VirtualVoice::spawn(channels.speech_out(), VirtualVoiceConfig::with_speed(8.0));
    let recorder = Arc::new(Recorder::default());
    let parts = parts(&talker, &recorder, Box::new(voice));
    let session = spawn(channels, parts);
    tokio::time::sleep(Duration::from_millis(100)).await;
    speak(&mut writer, 600, 0.5);
    speak(&mut writer, 400, 0.0);
    let answered = || recorder.marks().contains(&Milestone::FirstOp);
    assert!(run_until(10_000, answered).await, "{:?}", recorder.inputs());
    let inputs = recorder.inputs();
    let onset = inputs.iter().position(|input| *input == Input::VoiceOnset);
    let start = inputs.iter().position(|input| *input == Input::VoiceStart);
    assert!(onset < start && onset.is_some(), "{inputs:?}");
    assert!(inputs.contains(&Input::Transcript {
        text: "Walk me through it.".to_owned(),
        is_final: true
    }));
    let vocabulary = || {
        stt.vocabulary
            .lock()
            .map(|terms| terms.first().map(String::as_str) == Some("CDN"))
            .unwrap_or(false)
    };
    assert!(run_until(10_000, vocabulary).await, "{:?}", stt.vocabulary);
    let terms = stt
        .vocabulary
        .lock()
        .map(|terms| terms.clone())
        .unwrap_or_default();
    assert_eq!(terms.last().map(String::as_str), Some("SessionChannels"));
    // Muted: speech is not even heard.
    let mute = UiCommand::Mic {
        action: MicAction::Mute,
    };
    assert!(session.command(mute).is_ok());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = recorder.inputs().len();
    speak(&mut writer, 600, 0.5);
    speak(&mut writer, 400, 0.0);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let heard_while_muted: Vec<Input> = recorder.inputs()[before..]
        .iter()
        .filter(|input| matches!(input, Input::VoiceOnset | Input::VoiceStart))
        .cloned()
        .collect();
    assert!(heard_while_muted.is_empty(), "{heard_while_muted:?}");
    let _ = session.close().await;
    listener.shutdown().await;
}

/// A failure around the conversation (no audio, no microphone access) reaches the UI as a
/// `Problem`, even when reported before the session starts.
#[tokio::test(start_paused = true)]
async fn a_runtime_problem_reaches_the_ui() {
    let talker = Arc::new(ScriptedTalker::default());
    let channels = SessionChannels::new();
    channels.problems().report(
        "I can't hear you: microphone access is off for BoardAI.",
        "Turn it on in System Settings.",
    );
    let voice = VirtualVoice::spawn(channels.speech_out(), VirtualVoiceConfig::default());
    let recorder = Arc::new(Recorder::default());
    let parts = parts(&talker, &recorder, Box::new(voice));
    let session = spawn(channels, parts);
    let shown = || {
        recorder.events().iter().any(|event| {
            matches!(event, UiEvent::Problem { spoken, .. } if spoken.contains("microphone"))
        })
    };
    assert!(run_until(1_000, shown).await);
    let _ = session.close().await;
}

fn session_starts(recorder: &Recorder) -> usize {
    recorder
        .events()
        .iter()
        .filter(|event| matches!(event, UiEvent::SessionStart { .. }))
        .count()
}

/// The voice, the microphone and their links belong to the orchestrator, not to a session: after
/// an idle rest the next session's core speaks through the same voice, hears the same mic, and
/// ducks on its onsets. A bare onset (a cough) never wakes a resting session; a problem is shown
/// even while resting.
#[tokio::test(start_paused = true)]
async fn audio_outlives_a_rest_and_serves_the_next_session() {
    let talker = Arc::new(ScriptedTalker::default());
    talker.says(DEMO);
    talker.says(DEMO);
    let mut live = start_live(Arc::clone(&talker));
    let link = live.harness.session.speech_in();
    let recorder = Arc::clone(&live.harness.recorder);
    assert!(say_typed(&link, "Walk me through it.").await.is_ok());
    assert!(run_until(60_000, || idle_after_speaking(&recorder)).await);
    let first_answer = loud_cycles(&live.levels);
    assert!(first_answer > 0);
    tokio::time::sleep(Duration::from_millis(SESSION_IDLE_TIMEOUT_MS + 1_000)).await;
    assert_eq!(recorder.lifecycle(), vec!["start", "digest"], "resting");
    assert!(link.voice_onset().await.is_ok());
    live.problems
        .report("I can't hear you right now.", "while resting");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        session_starts(&recorder),
        1,
        "a cough does not wake the session"
    );
    let shown = recorder
        .events()
        .iter()
        .any(|event| matches!(event, UiEvent::Problem { detail, .. } if detail == "while resting"));
    assert!(shown, "a problem is shown while resting");
    assert!(say_typed(&link, "Where were we?").await.is_ok());
    let levels = Arc::clone(&live.levels);
    assert!(run_until(60_000, || loud_cycles(&levels) >= first_answer + 40).await);
    assert_eq!(
        session_starts(&recorder),
        2,
        "the words opened a new session"
    );
    assert!(
        live.listening.borrow_and_update().open,
        "the mic is open in it"
    );
    assert!(link.voice_onset().await.is_ok());
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(is_ducked(&live.gain), "the new core ducks the same voice");
    assert!(run_until(60_000, || idle_after_speaking(&recorder)).await);
    assert!(is_full(&live.gain));
    live.device.abort();
    let (stats, _, talker) = close(live.harness).await;
    assert_eq!(stats.sessions, 2);
    assert_eq!(talker.requests().len(), 2);
}
