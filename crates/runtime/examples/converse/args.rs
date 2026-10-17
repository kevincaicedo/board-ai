//! Command-line options for `converse`, parsed by hand (one dev tool does not earn a parser
//! dependency).

use std::path::PathBuf;

use boardai_runtime::limits::TALKER_MAX_TOKENS_DEFAULT;
use boardai_runtime::llm::ollama::OLLAMA_DEFAULT_MODEL;
use boardai_runtime::llm::openai::{OLLAMA_BASE_URL, OPENROUTER_BASE_URL};
use boardai_runtime::llm::openrouter::{self, ProviderSlug, TALKER_PRESET, parse_route};
use boardai_runtime::llm::{DEFAULT_TALKER_MODEL, Effort};
use boardai_runtime::settings::{LlmKind, LlmSettings, Secret, SecretName};

pub const USAGE: &str = "\
usage: just converse [options]

Type a line to say it; a line typed while BoardAI speaks is a barge-in. Ctrl-D ends.

  --provider ollama|anthropic|openrouter   default ollama
  --model <name>        default gemma4:latest (ollama), claude-opus-5-5 (anthropic),
                        openai/gpt-oss-120b (openrouter; qwen/qwen3.8-27b is the other preset)
  --effort none|low|medium|high|xhigh|max  default none for ollama, the preset's for
                        openrouter (gpt-oss: low, qwen: none), low otherwise
  --route <a,b|none>    OpenRouter providers to try first; default the preset's (cerebras)
  --max-tokens <n>      the talker's output budget; default the preset's (4096 on OpenRouter)
  --base-url <url>      override the provider's endpoint
  --no-warm-up          skip the talker's warm-up at session open (to measure it)
  --first-after <ms>    wait this long after the session opens before the first line, as a
                        user takes a moment to speak (default 0)
  --repo <path>         open a folder for the repo tools (read_file, search_code, list_dir)
  --script <file>       say the file's lines instead of reading stdin (`@<ms> text` times a
                        barge-in that long into the previous answer)
  --speed <x>           the virtual voice's speaking rate, default 1
  --verbose             print every caption update and debug logs
  --raw                 print each response's raw narration, tags included
  --ui-log <file>       write the UI events as an events.jsonl the UI's replay mode plays

Keys come from the environment: ANTHROPIC_API_KEY, and OPEN_ROUTER_KEY (or
OPENROUTER_API_KEY). `just converse` loads them from the repo's gitignored `.env`.";

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
    pub route: Option<Vec<ProviderSlug>>,
    pub max_tokens: Option<u32>,
    pub warm_up: bool,
    pub first_after_ms: u64,
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
            route: None,
            max_tokens: None,
            warm_up: true,
            first_after_ms: 0,
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
                "--no-warm-up" => {
                    args.warm_up = false;
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
                "--route" => args.route = Some(route(&value)?),
                "--max-tokens" => {
                    let tokens = value
                        .parse()
                        .map_err(|_| format!("bad --max-tokens {value}"))?;
                    args.max_tokens = Some(tokens);
                }
                "--first-after" => {
                    args.first_after_ms = value
                        .parse()
                        .map_err(|_| format!("bad --first-after {value}"))?;
                }
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
                Some(ANTHROPIC_KEYS),
            ),
            Provider::OpenRouter => (
                LlmKind::OpenAiCompatible,
                Some(TALKER_PRESET.model),
                OPENROUTER_BASE_URL,
                Some(OPEN_ROUTER_KEYS),
            ),
        };
        let model = self
            .model
            .clone()
            .or_else(|| default_model.map(str::to_owned))
            .ok_or("--model is required")?;
        let secret = key_variable.map(key_from_environment).transpose()?;
        let preset = openrouter::preset(&model).filter(|_| self.provider == Provider::OpenRouter);
        let default_effort = match (self.provider, preset) {
            (Provider::Ollama, _) => Effort::None,
            (Provider::OpenRouter, Some(preset)) => preset.talker.effort,
            (Provider::Anthropic | Provider::OpenRouter, _) => Effort::Low,
        };
        let max_tokens = self.max_tokens.unwrap_or_else(|| {
            preset.map_or(TALKER_MAX_TOKENS_DEFAULT, |preset| preset.talker.max_tokens)
        });
        let route = match (&self.route, preset) {
            (Some(route), _) => route.clone(),
            (None, Some(preset)) => preset.route(),
            (None, None) => Vec::new(),
        };
        let base_url = self
            .base_url
            .clone()
            .or_else(|| (!base_url.is_empty()).then(|| base_url.to_owned()));
        let settings = LlmSettings {
            kind,
            model,
            effort: self.effort.unwrap_or(default_effort),
            max_tokens,
            base_url,
            secret: secret.as_ref().map(|(name, _)| name.clone()),
            route,
        };
        Ok((settings, secret))
    }
}

/// Where a key comes from: the secret's name, and the variables tried in order.
type KeyVariables = (&'static str, &'static [&'static str]);

const ANTHROPIC_KEYS: KeyVariables = ("anthropic", &["ANTHROPIC_API_KEY"]);
const OPEN_ROUTER_KEYS: KeyVariables = ("openrouter", &["OPEN_ROUTER_KEY", "OPENROUTER_API_KEY"]);

/// The key from the first variable set. The value goes straight into a `Secret`; it is never
/// printed, and an error names the variables only.
fn key_from_environment((name, variables): KeyVariables) -> Result<(SecretName, Secret), String> {
    let value = variables
        .iter()
        .find_map(|variable| {
            std::env::var(variable)
                .ok()
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| format!("{} is not set", variables.join(" or ")))?;
    let name = SecretName::parse(name).ok_or("bad secret name")?;
    Ok((name, Secret::new(value.trim().to_owned())))
}

fn route(value: &str) -> Result<Vec<ProviderSlug>, String> {
    if value == "none" {
        return Ok(Vec::new());
    }
    let slugs: Vec<String> = value
        .split(',')
        .map(|slug| slug.trim().to_owned())
        .collect();
    parse_route(&slugs).map_err(|error| format!("bad --route: {error:?}"))
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
