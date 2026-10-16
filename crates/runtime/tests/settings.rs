//! `settings.toml`: defaults, the full shape, clear errors, and no secret ever in an error.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::path::{Path, PathBuf};

use boardai_runtime::audio::InputChoice;
use boardai_runtime::llm::Effort;
use boardai_runtime::settings::{
    HarnessChoice, LlmKind, MicMode, SecretName, Settings, SettingsError, TtsKind,
};
use support::TempFolder;

const HOME: &str = "/Users/someone";
const PLANTED_KEY: &str = "sk-ant-api03-PLANTEDkeyPLANTEDkeyPLANTEDkey-0123456789";

fn parse(text: &str) -> Result<Settings, SettingsError> {
    Settings::parse(text, Path::new(HOME))
}

fn invalid_field(text: &str) -> String {
    match parse(text) {
        Err(SettingsError::Invalid { field, .. }) => field,
        other => panic!("expected an invalid field for {text:?}, got {other:?}"),
    }
}

#[test]
fn an_empty_file_is_the_defaults() {
    let settings = parse("").expect("parses");
    assert_eq!(settings, Settings::defaults(Path::new(HOME)));
    assert_eq!(settings.talker.kind, LlmKind::Anthropic);
    assert_eq!(settings.talker.model, "claude-opus-5-5");
    assert_eq!(settings.talker.effort, Effort::Low);
    assert_eq!(settings.scribe.model, "claude-haiku-5-5");
    assert_eq!(settings.talker.secret, SecretName::parse("anthropic"));
    assert_eq!(settings.vault, PathBuf::from("/Users/someone/BoardAI"));
    assert_eq!(settings.mic, MicMode::Open);
    assert_eq!(settings.tts.kind, TtsKind::Kokoro);
    assert_eq!(settings.harness.kind, HarnessChoice::Auto);
    assert_eq!(settings.audio.input, InputChoice::SystemDefault);
}

#[test]
fn the_microphone_is_chosen_by_name_or_one_of_two_words() {
    let input = |text: &str| parse(text).expect("parses").audio.input;
    assert_eq!(input("[audio]\ninput = \"built_in\""), InputChoice::BuiltIn);
    assert_eq!(
        input("[audio]\ninput = \"system_default\""),
        InputChoice::SystemDefault
    );
    assert_eq!(
        input("[audio]\ninput = \"MacBook Pro Microphone\""),
        InputChoice::Named("MacBook Pro Microphone".to_owned())
    );
    assert_eq!(invalid_field("[audio]\ninput = \"  \""), "audio.input");
    assert!(
        parse("[audio]\noutput = \"x\"").is_err(),
        "unknown keys are refused"
    );
}

#[test]
fn every_role_can_be_configured() {
    let text = r#"
vault = "~/Notes/BoardAI"
code_roots = ["~/code", "/opt/src"]
mic = "push_to_talk"

[voice]
id = "af_heart"

[talker]
kind = "openai_compatible"
model = "qwen3.5:2b"
effort = "medium"
max_tokens = 4000

[scribe]
kind = "openai_compatible"
model = "anthropic/claude-haiku-5.5"
base_url = "https://openrouter.ai/api/v1"
secret = "openrouter"

[stt]
model = "small.en"

[tts]
kind = "elevenlabs"
model = "eleven_flash_v2_5"

[harness]
kind = "pi"
"#;
    let settings = parse(text).expect("parses");
    assert_eq!(
        settings.vault,
        PathBuf::from("/Users/someone/Notes/BoardAI")
    );
    assert_eq!(
        settings.code_roots,
        [
            PathBuf::from("/Users/someone/code"),
            PathBuf::from("/opt/src")
        ]
    );
    assert_eq!(settings.mic, MicMode::PushToTalk);
    assert_eq!(settings.voice.id, "af_heart");
    assert_eq!(settings.talker.kind, LlmKind::OpenAiCompatible);
    assert_eq!(
        settings.talker.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(settings.talker.secret, None);
    assert_eq!(
        (settings.talker.effort, settings.talker.max_tokens),
        (Effort::Medium, 4_000)
    );
    assert_eq!(settings.scribe.secret, SecretName::parse("openrouter"));
    assert_eq!(settings.stt.model, "small.en");
    assert_eq!(settings.tts.kind, TtsKind::ElevenLabs);
    assert_eq!(settings.tts.secret, SecretName::parse("elevenlabs"));
    assert_eq!(settings.harness.kind, HarnessChoice::Pi);
}

#[test]
fn errors_name_the_field() {
    let cases = [
        ("[talker]\nkind = \"gemini\"", "talker.kind"),
        ("[talker]\nkind = \"openai_compatible\"", "talker.model"),
        ("[scribe]\neffort = \"extreme\"", "scribe.effort"),
        ("[talker]\nmax_tokens = 0", "talker.max_tokens"),
        ("[talker]\nmax_tokens = 200000", "talker.max_tokens"),
        (
            "[talker]\nbase_url = \"ftp://example.com\"",
            "talker.base_url",
        ),
        (
            "[talker]\nbase_url = \"https://user:pw@example.com\"",
            "talker.base_url",
        ),
        ("[talker]\nmodel = \"  \"", "talker.model"),
        ("[talker]\nsecret = \"Has Spaces\"", "talker.secret"),
        ("vault = \"relative/path\"", "vault"),
        ("mic = \"always\"", "mic"),
        ("[stt]\nsecret = \"x\"", "stt.secret"),
        (
            "[tts]\nkind = \"kokoro\"\nbase_url = \"https://x.dev\"",
            "tts.base_url",
        ),
        ("[harness]\nkind = \"cursor\"", "harness.kind"),
    ];
    for (text, field) in cases {
        assert_eq!(invalid_field(text), field, "{text}");
    }
}

#[test]
fn syntax_errors_carry_a_line() {
    let error = parse("mic = \"open\"\n[talker\nkind = 1").expect_err("bad toml");
    assert!(
        matches!(error, SettingsError::Syntax { line: Some(2), .. }),
        "{error:?}"
    );
    let error = parse("[talker]\nmodel = 5").expect_err("wrong type");
    assert!(
        matches!(error, SettingsError::Syntax { line: Some(2), .. }),
        "{error:?}"
    );
}

#[test]
fn a_pasted_key_never_appears_in_an_error() {
    let attempts = [
        format!("api_key = \"{PLANTED_KEY}\""),
        format!("[talker]\napi_key = \"{PLANTED_KEY}\""),
        format!("[talker]\nsecret = \"{PLANTED_KEY}\""),
        format!("[talker]\nmax_tokens = \"{PLANTED_KEY}\""),
        format!("[talker]\nmodel = [\"{PLANTED_KEY}\"]"),
        format!("[talker]\nbase_url = \"{PLANTED_KEY}\""),
        format!("vault = \"{PLANTED_KEY}\""),
    ];
    for text in &attempts {
        let error = parse(text).expect_err("rejected");
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("PLANTED"), "{text} leaked: {shown}");
    }
    let error = parse(&format!("[talker]\nsecret = \"{PLANTED_KEY}\"")).expect_err("rejected");
    assert!(error.to_string().contains("Keychain"), "{error}");
}

#[test]
fn loading_reads_the_file_bounded_and_defaults_when_missing() {
    let folder = TempFolder::new("settings");
    let home = Path::new(HOME);
    let missing = Settings::load(&folder.path.join("settings.toml"), home).expect("defaults");
    assert_eq!(missing, Settings::defaults(home));
    let file = folder.write("settings.toml", b"mic = \"push_to_talk\"\n");
    assert_eq!(
        Settings::load(&file, home).expect("loads").mic,
        MicMode::PushToTalk
    );
    let huge = folder.write("huge.toml", &vec![b'#'; 300 * 1024]);
    assert!(matches!(
        Settings::load(&huge, home),
        Err(SettingsError::TooLarge { .. })
    ));
}
