//! Command-line options for `converse`, parsed by hand (one dev tool does not earn a parser
//! dependency).

use std::path::PathBuf;

use boardai_runtime::limits::TALKER_MAX_TOKENS_DEFAULT;
use boardai_runtime::llm::openai::{OLLAMA_BASE_URL, OPENROUTER_BASE_URL};
use boardai_runtime::llm::{DEFAULT_TALKER_MODEL, Effort};
use boardai_runtime::settings::{LlmKind, LlmSettings, Secret, SecretName};

pub const USAGE: &str = "\
usage: just converse [options]

Type a line to say it; a line typed while BoardAI speaks is a barge-in. Ctrl-D ends.

  --provider ollama|anthropic|openrouter   default ollama
  --model <name>        default gemma4:latest (ollama), claude-opus-5-5 (anthropic);
                        required for openrouter
  --effort none|low|medium|high|xhigh|max  default none for ollama, low otherwise
  --base-url <url>      override the provider's endpoint
  --repo <path>         open a folder for the repo tools (read_file, search_code, list_dir)
  --script <file>       say the file's lines instead of reading stdin (`@<ms> text` times a
                        barge-in that long into the previous answer)
  --speed <x>           the virtual voice's speaking rate, default 1
  --verbose             print every caption update and debug logs
  --raw                 print each response's raw narration, tags included
  --ui-log <file>       write the UI events as an events.jsonl the UI's replay mode plays

Keys come from the environment: ANTHROPIC_API_KEY, OPENROUTER_API_KEY.";

const OLLAMA_DEFAULT_MODEL: &str = "gemma4:latest";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    Anthropic,
    OpenRouter,
}

#[derive(Debug)]
pub struct Args {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub base_url: Option<String>,
    pub repo: Option<PathBuf>,
    pub script: Option<PathBuf>,
    pub speed: f32,
    pub verbose: bool,
    pub raw: bool,
    pub ui_log: Option<PathBuf>,
}

impl Args {
    pub fn parse(mut words: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = Self {
            provider: Provider::Ollama,
            model: None,
            effort: None,
            base_url: None,
            repo: None,
            script: None,
            speed: 1.0,
            verbose: false,
            raw: false,
            ui_log: None,
        };
        while let Some(flag) = words.next() {
            match flag.as_str() {
                "--verbose" => {
                    args.verbose = true;
                    continue;
                }
                "--raw" => {
                    args.raw = true;
                    continue;
                }
                _ => {}
            }
            let value = words
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--provider" => args.provider = provider(&value)?,
                "--model" => args.model = Some(value),
                "--effort" => args.effort = Some(effort(&value)?),
                "--base-url" => args.base_url = Some(value),
                "--repo" => args.repo = Some(PathBuf::from(value)),
                "--script" => args.script = Some(PathBuf::from(value)),
                "--ui-log" => args.ui_log = Some(PathBuf::from(value)),
                "--speed" => {
                    args.speed = value.parse().map_err(|_| format!("bad --speed {value}"))?;
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(args)
    }

    /// The talker settings these options describe, and the key they need from the environment.
    pub fn talker(&self) -> Result<(LlmSettings, Option<(SecretName, Secret)>), String> {
        let (kind, default_model, base_url, key_variable) = match self.provider {
            Provider::Ollama => (
                LlmKind::OpenAiCompatible,
                Some(OLLAMA_DEFAULT_MODEL),
                OLLAMA_BASE_URL,
                None,
            ),
            Provider::Anthropic => (
                LlmKind::Anthropic,
                Some(DEFAULT_TALKER_MODEL),
                "",
                Some("ANTHROPIC_API_KEY"),
            ),
            Provider::OpenRouter => (
                LlmKind::OpenAiCompatible,
                None,
                OPENROUTER_BASE_URL,
                Some("OPENROUTER_API_KEY"),
            ),
        };
        let model = self
            .model
            .clone()
            .or_else(|| default_model.map(str::to_owned))
            .ok_or("--model is required for openrouter")?;
        let secret = key_variable.map(key_from_environment).transpose()?;
        let default_effort = match self.provider {
            Provider::Ollama => Effort::None,
            Provider::Anthropic | Provider::OpenRouter => Effort::Low,
        };
        let base_url = self
            .base_url
            .clone()
            .or_else(|| (!base_url.is_empty()).then(|| base_url.to_owned()));
        let settings = LlmSettings {
            kind,
            model,
            effort: self.effort.unwrap_or(default_effort),
            max_tokens: TALKER_MAX_TOKENS_DEFAULT,
            base_url,
            secret: secret.as_ref().map(|(name, _)| name.clone()),
        };
        Ok((settings, secret))
    }
}

fn key_from_environment(variable: &str) -> Result<(SecretName, Secret), String> {
    let value = std::env::var(variable).map_err(|_| format!("{variable} is not set"))?;
    let name = SecretName::parse(&variable.to_ascii_lowercase().replace("_api_key", ""))
        .ok_or("bad secret name")?;
    Ok((name, Secret::new(value)))
}

fn provider(value: &str) -> Result<Provider, String> {
    match value {
        "ollama" => Ok(Provider::Ollama),
        "anthropic" => Ok(Provider::Anthropic),
        "openrouter" => Ok(Provider::OpenRouter),
        other => Err(format!("unknown provider {other}")),
    }
}

fn effort(value: &str) -> Result<Effort, String> {
    match value {
        "none" => Ok(Effort::None),
        "low" => Ok(Effort::Low),
        "medium" => Ok(Effort::Medium),
        "high" => Ok(Effort::High),
        "xhigh" => Ok(Effort::ExtraHigh),
        "max" => Ok(Effort::Max),
        other => Err(format!("unknown effort {other}")),
    }
}
