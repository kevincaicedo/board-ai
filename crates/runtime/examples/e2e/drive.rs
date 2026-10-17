//! Builds the pipeline, speaks the script into it at its cues, and hands the record to the
//! checks.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use boardai_core::history::{Conversation, Message, UserPart};
use boardai_core::time::SessionTime;
use boardai_core::{CoreConfig, MicMode, RepoReads};
use boardai_runtime::audio::{
    AudioConfig, AudioIo, ClipPlayed, FileMic, MIC_RING_FRAMES, NullOutput, VOICE_RING_FRAMES,
    VoiceOutput, mic_ring, voice_ring,
};
use boardai_runtime::bridge::UiSink;
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::{MonotonicClock, TokioClock};
use boardai_runtime::limits::{SCRIPT_WAIT_MAX_MS, TALKER_MAX_TOKENS_DEFAULT};
use boardai_runtime::llm::ollama::OLLAMA_DEFAULT_MODEL;
use boardai_runtime::llm::openai::{OLLAMA_BASE_URL, OPENROUTER_BASE_URL};
use boardai_runtime::llm::openrouter::{self, ProviderSlug, TALKER_PRESET, parse_route};
use boardai_runtime::llm::{ChatEvent, Effort, LlmProvider};
use boardai_runtime::models::ModelStore;
use boardai_runtime::session::{
    Cue, LiveListener, LiveVoice, Observed, Progress, Script, SessionChannels, SessionParts,
    SessionStats, StoreSink, talker_from_settings, wait_for_answer,
};
use boardai_runtime::settings::{
    LlmKind, LlmSettings, MemorySecretStore, Secret, SecretName, TtsKind, TtsSettings,
};
use boardai_runtime::speech_in::{SileroVad, SpeechIn, WhisperStt, load_speech_in, repo_terms};
use boardai_runtime::speech_out::{ChosenVoice, choose_voice};
use boardai_runtime::tools::RepoTools;
use futures_util::StreamExt;
use tokio::sync::{oneshot, watch};

use crate::check;
use crate::clips::{self, Clip};
use crate::trace::{Recorder, UiLog};
use crate::{Args, Output, Provider, Voice};

/// What keeps playing the voice: a null output, or the audio unit on the speakers.
enum Device {
    Null(NullOutput),
    /// Held, not read: dropping it stops the unit.
    Speakers {
        _unit: AudioIo,
    },
}

/// Everything loaded before the session starts, so loading never counts against latency.
struct Prepared {
    clips: Vec<Clip>,
    vad: SileroVad,
    stt: WhisperStt,
    voice: ChosenVoice,
    talker: Arc<dyn LlmProvider>,
}

async fn prepare(args: &Args) -> Result<Prepared, String> {
    let script = Script::parse(&args.script).map_err(|error| error.to_string())?;
    let say_voice = args.say_voice.clone();
    let clip_folder = args.out.join("clips");
    let clips = blocking(move || clips::speak(&script, &clip_folder, say_voice.as_deref())).await?;
    let models = ModelStore::new(models_root()?).map_err(|error| error.to_string())?;
    let speech_models = models.clone();
    let (vad, stt) = blocking(move || {
        load_speech_in(&speech_models, "base.en").map_err(|error| error.to_string())
    })
    .await?;
    let tts = tts_settings(args.voice);
    let voice = blocking(move || {
        choose_voice(&tts, "default", &MemorySecretStore::new(), &models)
            .map_err(|error| error.to_string())
    })
    .await?;
    if let Some(error) = &voice.replaced {
        println!("e2e: the configured voice could not start ({error}); using the system voice");
    }
    let (talker, settings) = talker(args)?;
    println!(
        "e2e: talker {} at effort {}{}",
        settings.model,
        settings.effort.as_str(),
        if settings.route.is_empty() {
            String::new()
        } else {
            let route: Vec<&str> = settings.route.iter().map(ProviderSlug::as_str).collect();
            format!(", routed to {}", route.join(", "))
        }
    );
    warm_up(&talker).await;
    Ok(Prepared {
        clips,
        vad,
        stt,
        voice,
        talker,
    })
}

/// Runs the script; true when every check passed.
pub async fn run(args: &Args) -> Result<bool, String> {
    let prepared = prepare(args).await?;
    let out = args.out.clone();
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    let channels = SessionChannels::new();
    let (output, device) = open_output(args.output)?;
    let voice = prepared.voice;
    let speech = LiveVoice::spawn(channels.speech_out(), voice.engine, voice.config, output);
    let (writer, reader) = mic_ring(MIC_RING_FRAMES);
    let (speech_in, inputs) =
        SpeechIn::spawn(reader, Box::new(prepared.vad), Box::new(prepared.stt))
            .map_err(|error| format!("speech in: {error}"))?;
    let listener = LiveListener::spawn(
        speech_in,
        inputs,
        channels.speech_in(),
        channels.listening(),
        vocabulary(args).await?,
    );
    let mic = FileMic::start(writer, 1.0).map_err(|error| format!("file mic: {error}"))?;
    let recorder = Arc::new(Recorder::default());
    let (store, mut progress) = Observed::new(Arc::clone(&recorder));
    let ui_log = Arc::new(UiLog::create(&out.join("events.jsonl"))?);
    let clock = TokioClock::start();
    let config = core_config(args.model.as_deref().unwrap_or(match args.provider {
        Provider::Ollama => OLLAMA_DEFAULT_MODEL,
        Provider::OpenRouter => TALKER_PRESET.model,
    }));
    let parts = SessionParts {
        core: config.clone(),
        clock,
        talker: prepared.talker,
        tools: RepoTools::default(),
        harness: None,
        speech: Box::new(speech),
        store: Arc::new(store) as Arc<dyn StoreSink>,
        ui: Arc::clone(&ui_log) as Arc<dyn UiSink>,
    };
    let session = boardai_runtime::session::spawn(channels, parts);
    let clips = prepared.clips;
    println!("e2e: {} lines; speaking them into the mic", clips.len());
    let played = say_all(&clips, &mic, &mut progress).await?;
    let stats = session.close().await;
    listener.shutdown().await;
    drop(mic);
    finish(&device, stats, &out);
    drop(device);
    ui_log.flush();
    recorder
        .write(&out.join("trace.jsonl"))
        .map_err(|error| format!("trace.jsonl: {error}"))?;
    let played: Vec<(SessionTime, SessionTime)> = played
        .iter()
        .map(|clip| {
            (
                clock.session_time_of(clip.started),
                clock.session_time_of(clip.ended),
            )
        })
        .collect();
    Ok(check::report(
        &recorder.entries(),
        &clips,
        &played,
        &config,
        args.strict,
    ))
}

fn finish(device: &Device, stats: SessionStats, out: &Path) {
    let voiced = match device {
        Device::Null(null) => format!("{} ms", null.voiced_frames() / 24),
        Device::Speakers { .. } => "on the speakers".to_owned(),
    };
    println!(
        "e2e: {} steps, {} stale talker inputs dropped, voice played {voiced}; wrote {}",
        stats.steps,
        stats.stale_talker_inputs,
        out.display()
    );
}

/// The terms Whisper starts with: `--vocabulary`, then the repo's symbols as the app takes them.
async fn vocabulary(args: &Args) -> Result<Vec<String>, String> {
    let mut terms = args.vocabulary.clone();
    if let Some(repo) = args.repo.clone() {
        terms.extend(blocking(move || Ok(repo_terms(std::slice::from_ref(&repo)))).await?);
    }
    if !terms.is_empty() {
        println!("e2e: Whisper expects {}", terms.join(", "));
    }
    Ok(terms)
}

/// Plays each clip at its cue, like a dev script does with typed lines, and returns when each
/// went into the mic.
async fn say_all(
    clips: &[Clip],
    mic: &FileMic,
    progress: &mut watch::Receiver<Progress>,
) -> Result<Vec<ClipPlayed>, String> {
    // Let the voice detector settle on silence first.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut answers: Vec<oneshot::Receiver<ClipPlayed>> = Vec::with_capacity(clips.len());
    let mut before: Option<Progress> = None;
    for clip in clips {
        if let Some(before) = before {
            wait_for_cue(clip.cue, before, progress).await;
        }
        let answer = mic
            .play(Arc::clone(&clip.samples))
            .map_err(|error| format!("file mic: {error:?}"))?;
        answers.push(answer);
        println!("e2e: said {:?}", clip.text);
        before = Some(*progress.borrow());
    }
    if let Some(before) = before {
        wait_for_answer(progress, before).await;
    }
    let mut played = Vec::with_capacity(answers.len());
    for answer in answers {
        played.push(answer.await.map_err(|_| "a clip never played".to_owned())?);
    }
    Ok(played)
}

async fn wait_for_cue(cue: Cue, before: Progress, progress: &mut watch::Receiver<Progress>) {
    let finished = move |now: &Progress| now.answers_finished > before.answers_finished;
    let started = move |now: &Progress| now.answers_started > before.answers_started;
    let limit = Duration::from_millis(SCRIPT_WAIT_MAX_MS);
    match cue {
        Cue::WhenIdle => wait_for_answer(progress, before).await,
        Cue::IntoSpeech { delay_ms } => {
            let waited = progress.wait_for(|now| started(now) || finished(now));
            if tokio::time::timeout(limit, waited).await.is_err() {
                println!("e2e: the answer never started; saying the next line anyway");
            }
            if !finished(&progress.borrow()) {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

fn open_output(output: Output) -> Result<(VoiceOutput, Device), String> {
    match output {
        Output::Null => {
            let (output, renderer) = voice_ring(VOICE_RING_FRAMES, 0);
            let null = NullOutput::start(renderer).map_err(|error| error.to_string())?;
            Ok((output, Device::Null(null)))
        }
        Output::Speakers => {
            // The unit's own mic is opened but unused: the file mic speaks for the user.
            let (audio, _mic, output) =
                AudioIo::start(AudioConfig::default()).map_err(|error| error.to_string())?;
            Ok((output, Device::Speakers { _unit: audio }))
        }
    }
}

fn tts_settings(voice: Voice) -> TtsSettings {
    TtsSettings {
        kind: match voice {
            Voice::System => TtsKind::System,
            Voice::Kokoro => TtsKind::Kokoro,
        },
        model: None,
        base_url: None,
        secret: None,
    }
}

fn core_config(model: &str) -> CoreConfig {
    CoreConfig {
        talker_name: model.to_owned(),
        mic: MicMode::OpenMic,
        session_context: None,
        suggestion: None,
        // The e2e opens no repo (`RepoTools::default()`).
        repo_reads: RepoReads::NotOffered,
    }
}

/// The talker `--provider` names: a local Ollama model at effort `none`, or an OpenRouter
/// model (a preset keeps its thinking and route) with the key from the environment.
fn talker(args: &Args) -> Result<(Arc<dyn LlmProvider>, LlmSettings), String> {
    let mut secrets = MemorySecretStore::new();
    let settings = match args.provider {
        Provider::Ollama => LlmSettings {
            kind: LlmKind::OpenAiCompatible,
            model: args
                .model
                .clone()
                .unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_owned()),
            effort: Effort::None,
            max_tokens: TALKER_MAX_TOKENS_DEFAULT,
            base_url: Some(OLLAMA_BASE_URL.to_owned()),
            secret: None,
            route: Vec::new(),
        },
        Provider::OpenRouter => {
            let model = args
                .model
                .clone()
                .unwrap_or_else(|| TALKER_PRESET.model.to_owned());
            let preset = openrouter::preset(&model);
            let route = match (args.route.as_deref(), preset) {
                (Some("none"), _) | (None, None) => Vec::new(),
                (Some(slugs), _) => {
                    let slugs: Vec<String> = slugs.split(',').map(str::to_owned).collect();
                    parse_route(&slugs).map_err(|error| format!("bad --route: {error:?}"))?
                }
                (None, Some(preset)) => preset.route(),
            };
            let name = SecretName::parse("openrouter").ok_or("bad secret name")?;
            let key = ["OPEN_ROUTER_KEY", "OPENROUTER_API_KEY"]
                .iter()
                .find_map(|variable| std::env::var(variable).ok().filter(|key| !key.is_empty()))
                .ok_or("OPEN_ROUTER_KEY or OPENROUTER_API_KEY is not set")?;
            secrets.insert(name.clone(), Secret::new(key.trim().to_owned()));
            LlmSettings {
                kind: LlmKind::OpenAiCompatible,
                effort: preset.map_or(Effort::Low, |preset| preset.talker.effort),
                model,
                max_tokens: args.max_tokens.unwrap_or_else(|| {
                    preset.map_or(TALKER_MAX_TOKENS_DEFAULT, |preset| preset.talker.max_tokens)
                }),
                base_url: Some(OPENROUTER_BASE_URL.to_owned()),
                secret: Some(name),
                route,
            }
        }
    };
    let clock = Arc::new(MonotonicClock::start());
    let talker = talker_from_settings(&settings, &secrets, clock).map_err(|e| e.to_string())?;
    Ok((talker, settings))
}

/// Warm-up requests before giving up: a cold Ollama loads the model (~5 s) and prefills the
/// system prompt (~5 s) behind the talker's 10 s first-token deadline, and keeps going after a
/// request that timed out, so the next one finds it ready.
const WARM_UP_ATTEMPTS: usize = 4;

/// Loads the model and its prompt before the clock matters, so line 1 measures a warm talker.
/// The answer is read to its end: a dropped OpenRouter stream may keep running (and holding
/// credit) at a provider that can't cancel, which can route line 1 to a slower provider.
async fn warm_up(talker: &Arc<dyn LlmProvider>) {
    let mut conversation = Conversation::new();
    conversation.push(Message::User {
        parts: vec![UserPart::Text {
            text: "Say OK.".to_owned(),
        }],
    });
    for attempt in 1..=WARM_UP_ATTEMPTS {
        let mut stream = talker.stream(&conversation, CancelToken::new());
        let answered = tokio::time::timeout(Duration::from_secs(60), async {
            let mut outcome = None;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(ChatEvent::Done { .. }) => return Ok(()),
                    Ok(ChatEvent::TextDelta(_) | ChatEvent::ToolCall(_)) => outcome = Some(()),
                    Err(error) => return Err(error.to_string()),
                }
            }
            outcome.ok_or_else(|| "the stream ended without an answer".to_owned())
        })
        .await;
        match answered {
            Ok(Ok(())) => return,
            Ok(Err(error)) => println!("e2e: the talker's warm-up {attempt} failed: {error}"),
            Err(_) => println!("e2e: the talker did not answer warm-up {attempt}"),
        }
    }
}

fn models_root() -> Result<PathBuf, String> {
    if let Some(folder) = std::env::var_os("BOARDAI_MODELS_DIR") {
        return Ok(PathBuf::from(folder));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(ModelStore::default_root(Path::new(&home)))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}
