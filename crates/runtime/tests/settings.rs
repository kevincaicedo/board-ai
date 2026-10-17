//! `settings.toml`: defaults, the full shape, clear errors, and no secret ever in an error.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::path::{Path, PathBuf};

use boardai_runtime::audio::{InputChoice, OutputChoice};
use boardai_runtime::limits::OPENROUTER_ROUTE_MAX;
use boardai_runtime::llm::Effort;
use boardai_runtime::llm::openrouter::{GPT_OSS_120B, ProviderSlug};
use boardai_runtime::settings::{
    HarnessChoice, LlmChoice, LlmKind, MicMode, SecretName, Settings, SettingsEdit, SettingsError,
    TtsKind,
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
    assert_eq!(
        settings.harness.research_model, None,
        "briefs run on sonnet"
    );
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
        parse("[audio]\nspeaker = \"x\"").is_err(),
        "unknown keys are refused"
    );
}

#[test]
fn the_speaker_follows_the_system_unless_one_is_named() {
    let output = |text: &str| parse(text).expect("parses").audio.output;
    assert_eq!(output(""), OutputChoice::SystemDefault);
    assert_eq!(
        output("[audio]\noutput = \"system_default\""),
        OutputChoice::SystemDefault
    );
    assert_eq!(
        output("[audio]\noutput = \"AirPods Pro\""),
        OutputChoice::Named("AirPods Pro".to_owned())
    );
    assert_eq!(invalid_field("[audio]\noutput = \"\""), "audio.output");
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
research_model = "opus"
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
    assert_eq!(settings.harness.research_model.as_deref(), Some("opus"));
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
        (
            "[harness]\nresearch_model = \" \"",
            "harness.research_model",
        ),
    ];
    for (text, field) in cases {
        assert_eq!(invalid_field(text), field, "{text}");
    }
}

/// Review 2, finding 10: `http://openrouter.ai` was accepted, so the key went out in
/// cleartext. A base URL that carries a key is https, unless it is this Mac.
#[test]
fn a_key_never_goes_to_a_plain_http_url_off_this_mac() {
    for text in [
        "[talker]\nkind = \"openai_compatible\"\nmodel = \"x\"\nbase_url = \"http://openrouter.ai/api/v1\"\nsecret = \"openrouter\"",
        "[scribe]\nkind = \"openai_compatible\"\nmodel = \"x\"\nbase_url = \"http://gpu.lan:8000/v1\"\nsecret = \"lan\"",
        "[talker]\nbase_url = \"http://api.anthropic.com\"",
        "[tts]\nkind = \"elevenlabs\"\nbase_url = \"http://api.elevenlabs.io\"",
    ] {
        let field = invalid_field(text);
        assert!(field.ends_with(".base_url"), "{text}: {field}");
    }
    for text in [
        "[talker]\nkind = \"openai_compatible\"\nmodel = \"x\"\nbase_url = \"http://localhost:1234/v1\"\nsecret = \"lm\"",
        "[talker]\nkind = \"openai_compatible\"\nmodel = \"x\"\nbase_url = \"http://127.0.0.1:8000/v1\"\nsecret = \"lm\"",
        "[talker]\nkind = \"openai_compatible\"\nmodel = \"x\"\nbase_url = \"http://gpu.lan:11434/v1\"",
        "[talker]\nkind = \"openai_compatible\"\nmodel = \"x\"\nbase_url = \"https://openrouter.ai/api/v1\"\nsecret = \"openrouter\"",
    ] {
        assert!(parse(text).is_ok(), "{text}");
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

#[test]
fn the_setup_flag_is_off_until_written() {
    assert!(!parse("").expect("parses").setup_complete);
    assert!(
        parse("setup_complete = true")
            .expect("parses")
            .setup_complete
    );
    assert!(matches!(
        parse("setup_complete = \"yes\""),
        Err(SettingsError::Syntax { .. })
    ));
}

fn ollama(model: &str) -> LlmChoice {
    LlmChoice {
        kind: LlmKind::OpenAiCompatible,
        model: Some(model.to_owned()),
        effort: Some(Effort::None),
        max_tokens: None,
        base_url: None,
        secret: None,
        route: Vec::new(),
        early_start: None,
    }
}

#[test]
fn an_edit_creates_the_file_and_keeps_what_it_does_not_touch() {
    let folder = TempFolder::new("settings-edit");
    let home = Path::new(HOME);
    let path = folder.path.join("config/settings.toml");
    let first = Settings::edit(&path, home, &[SettingsEdit::Mic(MicMode::PushToTalk)])
        .expect("creates the file and its folder");
    assert_eq!(first.mic, MicMode::PushToTalk);
    assert_eq!(
        std::fs::read_to_string(&path).expect("written"),
        "mic = \"push_to_talk\"\n"
    );

    std::fs::write(
        &path,
        "vault = \"~/Notes\"\ncode_roots = [\"~/code\"]\n[harness]\nkind = \"pi\"\n[talker]\nmax_tokens = 900\n",
    )
    .expect("hand-written");
    let edits = [
        SettingsEdit::Llm {
            talker: ollama("gemma4:latest"),
            scribe: ollama("gemma4:latest"),
        },
        SettingsEdit::Tts(TtsKind::System),
        SettingsEdit::Input(InputChoice::BuiltIn),
        SettingsEdit::SetupComplete,
    ];
    let edited = Settings::edit(&path, home, &edits).expect("edited");
    assert_eq!(
        edited,
        Settings::load(&path, home).expect("reloads the same")
    );
    assert_eq!(edited.vault, PathBuf::from("/Users/someone/Notes"));
    assert_eq!(
        edited.code_roots,
        vec![PathBuf::from("/Users/someone/code")]
    );
    assert_eq!(edited.harness.kind, HarnessChoice::Pi);
    assert_eq!(edited.talker.kind, LlmKind::OpenAiCompatible);
    assert_eq!(edited.talker.model, "gemma4:latest");
    assert_eq!(edited.talker.effort, Effort::None);
    // A new provider starts from its own defaults: the old output budget goes with the old one.
    assert_eq!(
        edited.talker.max_tokens,
        Settings::defaults(home).talker.max_tokens
    );
    assert_eq!(edited.tts.kind, TtsKind::System);
    assert_eq!(edited.audio.input, InputChoice::BuiltIn);
    assert!(edited.setup_complete);
}

#[test]
fn edits_switch_back_to_the_defaults_and_pick_a_microphone_by_name() {
    let folder = TempFolder::new("settings-switch");
    let home = Path::new(HOME);
    let path = folder.path.join("config/settings.toml");
    let local = SettingsEdit::Llm {
        talker: ollama("gemma4:latest"),
        scribe: ollama("gemma4:latest"),
    };
    Settings::edit(&path, home, &[local]).expect("edited");
    let claude = LlmChoice {
        kind: LlmKind::Anthropic,
        model: None,
        effort: None,
        max_tokens: None,
        base_url: None,
        secret: SecretName::parse("anthropic"),
        route: Vec::new(),
        early_start: None,
    };
    let back = Settings::edit(
        &path,
        home,
        &[SettingsEdit::Llm {
            talker: claude.clone(),
            scribe: claude,
        }],
    )
    .expect("edited");
    assert_eq!(back.talker, Settings::defaults(home).talker);
    assert_eq!(back.scribe, Settings::defaults(home).scribe);
    let named = InputChoice::Named("Studio Display Microphone".to_owned());
    let speaker = "AirPods Pro";
    let text = std::fs::read_to_string(&path).expect("written");
    std::fs::write(&path, format!("{text}\n[audio]\noutput = \"{speaker}\"\n")).expect("write");
    let picked =
        Settings::edit(&path, home, &[SettingsEdit::Input(named.clone())]).expect("edited");
    assert_eq!(picked.audio.input, named);
    assert_eq!(
        picked.audio.output,
        OutputChoice::Named(speaker.to_owned()),
        "picking a microphone keeps a hand-written speaker"
    );
    assert_eq!(
        Settings::load(&path, home).expect("reloads").audio.input,
        named
    );
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().expect("folder"))
        .expect("listed")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("settings.toml")]);
}

#[test]
fn an_edit_never_overwrites_a_file_that_does_not_parse_or_writes_an_invalid_one() {
    let folder = TempFolder::new("settings-refused");
    let home = Path::new(HOME);
    let broken = "# my notes\n[talker]\nkind = \"gemini\"\n";
    let path = folder.write("settings.toml", broken.as_bytes());
    let refused = Settings::edit(&path, home, &[SettingsEdit::SetupComplete]);
    assert!(matches!(refused, Err(SettingsError::Invalid { .. })));
    assert_eq!(std::fs::read_to_string(&path).expect("kept"), broken);
    // Even an edit that would replace the bad field leaves a hand-written file alone.
    let half_done = "# trying push to talk\nmic = \"sometimes\"\n";
    let path = folder.write("half-done.toml", half_done.as_bytes());
    let refused = Settings::edit(&path, home, &[SettingsEdit::Mic(MicMode::Open)]);
    assert!(matches!(refused, Err(SettingsError::Invalid { .. })));
    assert_eq!(std::fs::read_to_string(&path).expect("kept"), half_done);

    let valid = folder.write("valid.toml", b"mic = \"open\"\n");
    let empty_model = SettingsEdit::Llm {
        talker: ollama("  "),
        scribe: ollama("gemma4:latest"),
    };
    let refused = Settings::edit(&valid, home, &[empty_model]);
    assert!(
        matches!(&refused, Err(SettingsError::Invalid { field, .. }) if field == "talker.model"),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&valid).expect("kept"),
        "mic = \"open\"\n"
    );
}

#[test]
fn an_edit_that_cannot_write_reports_it_and_keeps_the_old_file() {
    use std::os::unix::fs::PermissionsExt as _;
    let folder = TempFolder::new("settings-unwritable");
    let home = Path::new(HOME);
    let path = folder.write("config/settings.toml", b"mic = \"open\"\n");
    let config = path.parent().expect("folder").to_path_buf();
    let read_only = std::fs::Permissions::from_mode(0o555);
    std::fs::set_permissions(&config, read_only).expect("read-only folder");
    let refused = Settings::edit(&path, home, &[SettingsEdit::Mic(MicMode::PushToTalk)]);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).expect("restored");
    assert!(
        matches!(refused, Err(SettingsError::Unwritable { .. })),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("kept"),
        "mic = \"open\"\n"
    );
    let names: Vec<_> = std::fs::read_dir(&config)
        .expect("listed")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("settings.toml")]);
}

const OPEN_ROUTER_TALKER: &str = "[talker]\nkind = \"openai_compatible\"\nmodel = \"openai/gpt-oss-120b\"\nbase_url = \"https://openrouter.ai/api/v1\"\nsecret = \"openrouter\"\n";

#[test]
fn an_openrouter_route_is_read_and_bounded() {
    let settings = parse(&format!(
        "{OPEN_ROUTER_TALKER}route = [\"cerebras\", \"deepinfra/turbo\"]"
    ))
    .expect("parses");
    let route: Vec<&str> = settings
        .talker
        .route
        .iter()
        .map(ProviderSlug::as_str)
        .collect();
    assert_eq!(route, ["cerebras", "deepinfra/turbo"]);
    assert!(
        parse(OPEN_ROUTER_TALKER)
            .expect("parses")
            .talker
            .route
            .is_empty()
    );
    assert_eq!(
        invalid_field(&format!(
            "{OPEN_ROUTER_TALKER}route = [\"cerebras\", \"Groq Cloud\"]"
        )),
        "talker.route[1]"
    );
    let long: Vec<String> = (0..=OPENROUTER_ROUTE_MAX)
        .map(|_| "\"groq\"".to_owned())
        .collect();
    assert_eq!(
        invalid_field(&format!(
            "{OPEN_ROUTER_TALKER}route = [{}]",
            long.join(", ")
        )),
        "talker.route"
    );
    // A route means nothing to other servers: saying so beats ignoring it.
    let local =
        "[talker]\nkind = \"openai_compatible\"\nmodel = \"gemma4:latest\"\nroute = [\"cerebras\"]";
    assert_eq!(invalid_field(local), "talker.route");
    assert_eq!(
        invalid_field("[scribe]\nroute = [\"cerebras\"]"),
        "scribe.route"
    );
}

#[test]
fn a_route_error_never_quotes_the_slug() {
    let text = format!("{OPEN_ROUTER_TALKER}route = [\"{PLANTED_KEY}\"]");
    let error = parse(&text).expect_err("a key is not a slug").to_string();
    assert!(!error.contains(&PLANTED_KEY[..20]), "{error}");
}

#[test]
fn an_edit_writes_a_preset_with_its_route() {
    let folder = TempFolder::new("settings-route");
    let home = Path::new(HOME);
    let path = folder.path.join("config/settings.toml");
    let open_router = LlmChoice {
        kind: LlmKind::OpenAiCompatible,
        model: Some(GPT_OSS_120B.model.to_owned()),
        effort: Some(GPT_OSS_120B.talker.effort),
        max_tokens: Some(GPT_OSS_120B.talker.max_tokens),
        base_url: Some("https://openrouter.ai/api/v1".to_owned()),
        secret: SecretName::parse("openrouter"),
        route: GPT_OSS_120B.route(),
        early_start: None,
    };
    let edited = Settings::edit(
        &path,
        home,
        &[SettingsEdit::Llm {
            talker: open_router.clone(),
            scribe: open_router,
        }],
    )
    .expect("edited");
    assert_eq!(edited.talker.route, GPT_OSS_120B.route());
    assert_eq!(edited.talker.effort, Effort::Low);
    assert_eq!(edited.talker.max_tokens, 4_096);
    let written = std::fs::read_to_string(&path).expect("written");
    assert!(written.contains("route = [\"cerebras\"]"), "{written}");
    assert_eq!(Settings::load(&path, home).expect("reloads"), edited);
}

/// D-262: a hosted talker starts a request on the early final unless `[talker] early_start =
/// false`; the scribe never talks, so its section refuses the field.
#[test]
fn the_early_start_is_on_until_the_talker_turns_it_off() {
    assert!(parse("").expect("parses").talker.early_start);
    let off = parse(&format!("{OPEN_ROUTER_TALKER}early_start = false")).expect("parses");
    assert!(!off.talker.early_start);
    assert!(off.scribe.early_start);
    let on = parse(&format!("{OPEN_ROUTER_TALKER}early_start = true")).expect("parses");
    assert!(on.talker.early_start);
    assert_eq!(
        invalid_field("[scribe]\nearly_start = false"),
        "scribe.early_start"
    );
    assert!(parse("[talker]\nearly_start = \"no\"").is_err());
}

/// The edit writes the field, and picking another model keeps the user's `early_start = false`.
#[test]
fn an_edit_writes_the_early_start_and_a_new_model_keeps_it() {
    let folder = TempFolder::new("settings-early-start");
    let home = Path::new(HOME);
    let path = folder.path.join("settings.toml");
    let choice = |early_start| LlmChoice {
        kind: LlmKind::OpenAiCompatible,
        model: Some(GPT_OSS_120B.model.to_owned()),
        effort: None,
        max_tokens: None,
        base_url: Some("https://openrouter.ai/api/v1".to_owned()),
        secret: SecretName::parse("openrouter"),
        route: GPT_OSS_120B.route(),
        early_start,
    };
    let edit = |talker: LlmChoice| SettingsEdit::Llm {
        talker,
        scribe: ollama("qwen3.5:2b"),
    };
    let off = Settings::edit(&path, home, &[edit(choice(Some(false)))]).expect("edited");
    assert!(!off.talker.early_start);
    let written = std::fs::read_to_string(&path).expect("written");
    assert!(written.contains("early_start = false"), "{written}");
    let kept = Settings::edit(&path, home, &[edit(choice(None))]).expect("edited");
    assert!(!kept.talker.early_start, "a new pick keeps it off");
    assert_eq!(Settings::load(&path, home).expect("reloads"), kept);
    let on = Settings::edit(&path, home, &[edit(choice(Some(true)))]).expect("edited");
    assert!(on.talker.early_start);
}

#[test]
fn the_scribe_and_the_code_folders_change_alone() {
    let folder = TempFolder::new("settings-scribe-roots");
    let home = Path::new(HOME);
    let path = folder.path.join("settings.toml");
    std::fs::write(
        &path,
        "code_roots = [\"~/code\", \"/opt/src\"]\n[talker]\nkind = \"openai_compatible\"\nmodel = \"gemma4:latest\"\neffort = \"none\"\n",
    )
    .expect("hand-written");
    let edits = [
        SettingsEdit::Scribe(ollama("qwen3.5:2b")),
        SettingsEdit::AddCodeRoot("~/Documents/Projects".to_owned()),
        SettingsEdit::RemoveCodeRoot(0),
    ];
    let edited = Settings::edit(&path, home, &edits).expect("edited");
    assert_eq!(edited.talker.model, "gemma4:latest", "the talker stays");
    assert_eq!(edited.scribe.model, "qwen3.5:2b");
    assert_eq!(
        edited.code_roots,
        vec![
            PathBuf::from("/opt/src"),
            PathBuf::from("/Users/someone/Documents/Projects")
        ]
    );
    let text = std::fs::read_to_string(&path).expect("written");
    assert!(text.contains("code_roots = [\"/opt/src\", \"~/Documents/Projects\"]"));
    // Past the end removes nothing; a file without folders gets its first one.
    let unchanged = Settings::edit(&path, home, &[SettingsEdit::RemoveCodeRoot(9)]).expect("ok");
    assert_eq!(unchanged.code_roots.len(), 2);
    let empty = folder.path.join("empty.toml");
    let added = Settings::edit(
        &empty,
        home,
        &[SettingsEdit::AddCodeRoot("/srv".to_owned())],
    )
    .expect("created");
    assert_eq!(added.code_roots, vec![PathBuf::from("/srv")]);
    let relative = Settings::edit(&empty, home, &[SettingsEdit::AddCodeRoot("src".to_owned())]);
    assert!(
        matches!(relative, Err(SettingsError::Invalid { field, .. }) if field == "code_roots"),
        "a folder that isn't absolute or under ~/ is refused and the file kept"
    );
    assert_eq!(
        Settings::load(&empty, home)
            .expect("still valid")
            .code_roots,
        vec![PathBuf::from("/srv")]
    );
}
