//! The start rule and the wire shapes of the setup types.

use super::{
    ElevenLabsVoice, FileState, HarnessFound, HarnessName, HarnessesFound, InputDevice, KeyName,
    KeyState, KeyStatus, MicChoice, MicPermission, ModelFileStatus, ModelInfo, ModelListing,
    ModelPrice, ModelProvider, ModelQuery, ModelReasoning, OllamaStatus, SetupBlocker, SetupEdit,
    SetupStatus, TalkerProvider, TalkerSetup, ThinkingEffort, VoiceEngine, blockers,
};

fn file(id: &str, state: FileState) -> ModelFileStatus {
    ModelFileStatus {
        id: id.to_owned(),
        state,
        have_bytes: 0,
        size_bytes: 10,
    }
}

fn ready() -> SetupStatus {
    SetupStatus {
        complete: false,
        settings_problem: None,
        talker: TalkerSetup {
            provider: TalkerProvider::Claude,
            model: "claude-opus-5-5".to_owned(),
            key: KeyState::Saved,
            ollama: OllamaStatus::Unreachable,
            thinking: ThinkingEffort::Low,
            route: Vec::new(),
            preset: false,
        },
        scribe_model: "claude-haiku-5-5".to_owned(),
        keys: vec![KeyStatus {
            name: KeyName::Anthropic,
            state: KeyState::Saved,
        }],
        voice: VoiceEngine::Kokoro,
        elevenlabs: ElevenLabsVoice {
            model: "Flash".to_owned(),
            voice: "Rachel".to_owned(),
        },
        voice_files: vec![file("kokoro", FileState::Missing)],
        hearing_model: "base.en".to_owned(),
        hearing_files: vec![
            file("whisper-base.en", FileState::Ready),
            file("silero-vad", FileState::Ready),
        ],
        microphone: MicPermission::Granted,
        mic_mode: MicChoice::Open,
        input: InputDevice::SystemDefault,
        input_devices: Vec::new(),
        default_input: None,
        built_in_input: None,
        harnesses: HarnessesFound {
            claude_code: HarnessFound::Found { version: None },
            pi: HarnessFound::Missing,
            default: Some(HarnessName::ClaudeCode),
        },
        code_folders: Vec::new(),
        vault: "~/BoardAI".to_owned(),
        blockers: Vec::new(),
        mic_attributed_to: None,
    }
}

#[test]
fn a_saved_key_ready_hearing_and_the_mic_start_even_while_the_voice_downloads() {
    assert_eq!(blockers(&ready()), Vec::new());
}

#[test]
fn every_missing_piece_is_named_in_order() {
    let mut status = ready();
    status.settings_problem = Some("settings.toml `mic`: must be …".to_owned());
    status.talker.key = KeyState::Missing;
    status.hearing_files[0].state = FileState::Downloading;
    status.microphone = MicPermission::Denied;
    assert_eq!(
        blockers(&status),
        vec![
            SetupBlocker::SettingsProblem,
            SetupBlocker::TalkerKey {
                provider: TalkerProvider::Claude
            },
            SetupBlocker::HearingNotReady,
            SetupBlocker::MicrophoneDenied,
        ]
    );
    status.microphone = MicPermission::Undetermined;
    assert!(blockers(&status).contains(&SetupBlocker::MicrophoneNotAllowed));
}

#[test]
fn push_to_talk_needs_the_hearing_files_too() {
    let mut status = ready();
    status.mic_mode = MicChoice::PushToTalk;
    status.hearing_files[1].state = FileState::Partial;
    assert_eq!(blockers(&status), vec![SetupBlocker::HearingNotReady]);
}

#[test]
fn ollama_needs_to_answer_with_the_configured_model() {
    let mut status = ready();
    status.talker = TalkerSetup {
        provider: TalkerProvider::Ollama,
        model: "gemma4:latest".to_owned(),
        key: KeyState::NotNeeded,
        ollama: OllamaStatus::Unreachable,
        thinking: ThinkingEffort::Off,
        route: Vec::new(),
        preset: true,
    };
    assert_eq!(blockers(&status), vec![SetupBlocker::OllamaUnreachable]);
    status.talker.ollama = OllamaStatus::Reachable {
        models: vec!["qwen3.5:2b".to_owned()],
    };
    assert_eq!(
        blockers(&status),
        vec![SetupBlocker::OllamaModelMissing {
            model: "gemma4:latest".to_owned()
        }]
    );
    status.talker.ollama = OllamaStatus::Reachable {
        models: vec!["qwen3.5:2b".to_owned(), "gemma4:latest".to_owned()],
    };
    assert_eq!(blockers(&status), Vec::new());
}

#[test]
fn an_unknown_whisper_model_is_named() {
    let mut status = ready();
    status.hearing_model = "large".to_owned();
    status.hearing_files.truncate(1);
    assert_eq!(
        blockers(&status),
        vec![SetupBlocker::UnknownHearingModel {
            model: "large".to_owned()
        }]
    );
}

#[test]
fn edits_from_the_ui_are_parsed_and_unknown_ones_refused() {
    let edit: Result<SetupEdit, _> =
        serde_json::from_str(r#"{"kind":"Talker","provider":"Ollama","model":"gemma4:latest"}"#);
    assert!(matches!(
        edit,
        Ok(SetupEdit::Talker {
            provider: TalkerProvider::Ollama,
            ..
        })
    ));
    for bad in [
        r#"{"kind":"Talker","provider":"Gemini","model":null}"#,
        r#"{"kind":"Delete"}"#,
        r#"{"kind":"Mic","mode":"Always"}"#,
        r#"{"kind":"Input","device":{"kind":"Named"}}"#,
    ] {
        assert!(serde_json::from_str::<SetupEdit>(bad).is_err(), "{bad}");
    }
    assert!(serde_json::from_str::<KeyName>(r#""GitHub""#).is_err());
}

#[test]
fn model_queries_from_the_ui_are_parsed_and_unknown_ones_refused() {
    let providers: Result<ModelQuery, _> =
        serde_json::from_str(r#"{"kind":"Providers","model":"qwen/qwen3.8-27b"}"#);
    assert!(matches!(providers, Ok(ModelQuery::Providers { .. })));
    assert_eq!(
        serde_json::from_str::<ModelQuery>(r#"{"kind":"OpenRouter"}"#).ok(),
        Some(ModelQuery::OpenRouter)
    );
    for bad in [
        r#"{"kind":"Anthropic"}"#,
        r#"{"kind":"Providers"}"#,
        r#"{"kind":"Providers","model":7}"#,
    ] {
        assert!(serde_json::from_str::<ModelQuery>(bad).is_err(), "{bad}");
    }
    let listing = ModelListing::OpenRouter {
        models: vec![ModelInfo {
            id: "openai/gpt-oss-120b".to_owned(),
            name: "OpenAI: gpt-oss-120b".to_owned(),
            context_tokens: Some(131_072),
            price: Some(ModelPrice {
                input_micro_usd_per_million_tokens: 150_000,
                output_micro_usd_per_million_tokens: 600_000,
            }),
            reasoning: ModelReasoning::Always {
                least_effort: "low".to_owned(),
            },
            preset: true,
            route: vec!["cerebras".to_owned()],
        }],
        left_out: 0,
    };
    let wire = serde_json::to_value(&listing).unwrap_or_default();
    assert_eq!(wire["kind"], "OpenRouter");
    assert_eq!(wire["models"][0]["reasoning"]["kind"], "Always");
    assert_eq!(
        wire["models"][0]["price"]["input_micro_usd_per_million_tokens"],
        150_000
    );
    assert_eq!(wire["models"][0]["route"][0], "cerebras");
    let providers = ModelListing::Providers {
        model: "qwen/qwen3.8-27b".to_owned(),
        providers: vec![ModelProvider {
            name: "Cerebras".to_owned(),
            slug: "cerebras/fp16".to_owned(),
            context_tokens: Some(65_536),
            max_output_tokens: None,
            price: None,
            quantization: Some("fp16".to_owned()),
            tools: true,
            latency_ms_p50: None,
            throughput_tokens_per_second_p50: None,
        }],
        left_out: 0,
    };
    let wire = serde_json::to_value(&providers).unwrap_or_default();
    assert!(
        wire["providers"][0]["latency_ms_p50"].is_null(),
        "an unknown speed is sent as unknown, never as zero"
    );
}

#[test]
fn settings_edits_name_a_model_or_a_folder_by_position_never_a_path() {
    let scribe: Result<SetupEdit, _> =
        serde_json::from_str(r#"{"kind":"Scribe","model":"qwen/qwen3.8-27b"}"#);
    assert!(matches!(scribe, Ok(SetupEdit::Scribe { .. })));
    let remove: Result<SetupEdit, _> =
        serde_json::from_str(r#"{"kind":"RemoveCodeFolder","index":1}"#);
    assert_eq!(remove.ok(), Some(SetupEdit::RemoveCodeFolder { index: 1 }));
    for bad in [
        r#"{"kind":"RemoveCodeFolder","index":-1}"#,
        r#"{"kind":"RemoveCodeFolder","index":"~/Projects"}"#,
        r#"{"kind":"AddCodeFolder","path":"/etc"}"#,
        r#"{"kind":"Scribe"}"#,
    ] {
        assert!(serde_json::from_str::<SetupEdit>(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_harness_is_missing_or_found_with_its_version_when_it_answered() {
    let found = serde_json::to_value(HarnessFound::Found {
        version: Some("2.1.301".to_owned()),
    })
    .unwrap_or_default();
    assert_eq!(found["state"], "Found");
    assert_eq!(found["version"], "2.1.301");
    let missing = serde_json::to_value(HarnessFound::Missing).unwrap_or_default();
    assert_eq!(missing["state"], "Missing");
}

#[test]
fn an_unreadable_keychain_blocks_like_a_missing_key() {
    let mut status = ready();
    status.talker.key = KeyState::Unreadable;
    assert_eq!(
        blockers(&status),
        vec![SetupBlocker::TalkerKey {
            provider: TalkerProvider::Claude
        }]
    );
}
