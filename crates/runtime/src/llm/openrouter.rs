//! OpenRouter (LLD §3.3): the presets BoardAI is measured with, provider routing, and the
//! `reasoning` setting OpenRouter documents for every model (D-152).
//!
//! OpenRouter speaks the OpenAI-compatible protocol, so [`super::openai`] renders its requests;
//! this module holds what differs: a route (`provider.order`) and the `reasoning` object in
//! place of `reasoning_effort`.

use boardai_core::setup::{Drawing, DrawingMeasure};

use super::{Effort, LlmRole};
use crate::limits::{
    OPENROUTER_ROUTE_MAX, OPENROUTER_SLUG_MAX_BYTES, OPENROUTER_TALKER_MAX_TOKENS,
    SCRIBE_MAX_TOKENS_DEFAULT,
};

/// OpenRouter's host; a base URL on it gets OpenRouter's request shape.
pub const OPENROUTER_HOST: &str = "openrouter.ai";

/// Whether `base_url` is OpenRouter's API.
#[must_use]
pub fn is_openrouter(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host == OPENROUTER_HOST))
        .unwrap_or(false)
}

/// A provider as OpenRouter's routing names it: `cerebras`, or a variant such as
/// `deepinfra/turbo`. 1 to [`OPENROUTER_SLUG_MAX_BYTES`] bytes of lowercase ASCII letters,
/// digits and `- _ . /`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderSlug(String);

impl ProviderSlug {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid_byte = |byte: u8| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b'/')
        };
        let valid = !text.is_empty()
            && text.len() <= OPENROUTER_SLUG_MAX_BYTES
            && text.bytes().all(valid_byte)
            && !text.starts_with('/')
            && !text.ends_with('/');
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the provider OpenRouter names in a response (`Cerebras`, `Amazon Bedrock`) is
    /// this one: its display name and the slug's first part spell the same letters and digits
    /// (`AtlasCloud` is `atlas-cloud`), or the name is one OpenRouter gives a slug that spells
    /// something else ([`SERVED_BY_NAMES`]).
    #[must_use]
    pub fn names(&self, provider_name: &str) -> bool {
        let base = self.0.split('/').next().unwrap_or_default();
        let spelled = |text: &str| -> String {
            text.chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|letter| letter.to_ascii_lowercase())
                .collect()
        };
        let shown = spelled(provider_name);
        if base.is_empty() || shown.is_empty() {
            return false;
        }
        shown == spelled(base)
            || SERVED_BY_NAMES
                .iter()
                .any(|(slug, name)| *slug == base && spelled(name) == shown)
    }
}

/// Route slugs whose provider OpenRouter names otherwise in a response, from its endpoint
/// lists (2026-10-16). Without them a strict route to Google Vertex or Claude Platform on AWS
/// warned on every answer that another provider had served it.
const SERVED_BY_NAMES: [(&str, &str); 3] = [
    ("google-vertex", "Google"),
    ("claude-on-aws", "Claude Platform on AWS"),
    ("mancer", "Mancer 2"),
];

/// A model BoardAI offers on OpenRouter with the settings it was measured with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub model: &'static str,
    /// The only providers tried, in order (`provider.order`, no fallbacks, D-152).
    pub route: &'static [&'static str],
    /// As the talker: thinking off where the model allows it, else the least it takes, and
    /// [`OPENROUTER_TALKER_MAX_TOKENS`].
    pub talker: RoleSettings,
    /// As the scribe: digests are not latency-bound, so it thinks at `medium` (D-017's scribe
    /// effort) with the scribe's usual budget.
    pub scribe: RoleSettings,
    /// Whether the request asks the provider to cache the prompt's prefix.
    pub prompt_cache: PromptCache,
    /// How it drew as the talker in spike S5, when measured (D-219).
    pub drawing: Option<DrawingMeasured>,
}

/// What spike S5 measured for a preset as the talker (HLD §11): `converse` on the four standard
/// scripts, two runs each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawingMeasured {
    /// Answers that drew at least one shape.
    pub drew: u32,
    pub answers: u32,
    /// From the request to the first stroke, median over the answers that drew.
    pub first_stroke_ms_p50: u32,
    pub measured_on: &'static str,
}

impl DrawingMeasured {
    /// As the settings page gets it.
    #[must_use]
    pub fn to_setup(self) -> DrawingMeasure {
        DrawingMeasure {
            drew: self.drew,
            answers: self.answers,
            first_stroke_ms_p50: self.first_stroke_ms_p50,
            measured_on: self.measured_on.to_owned(),
        }
    }
}

/// HLD §11's measures, all from 2026-10-16.
const fn measured(drew: u32, first_stroke_ms_p50: u32) -> Option<DrawingMeasured> {
    Some(DrawingMeasured {
        drew,
        answers: 12,
        first_stroke_ms_p50,
        measured_on: "2026-10-16",
    })
}

/// How a model's providers reuse the frozen prefix (system prompt and tools, LLD §3.3) from one
/// request to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptCache {
    /// The provider caches a repeated prefix by itself (Cerebras, OpenAI, DeepSeek's hosts,
    /// Gemini): nothing is sent.
    Automatic,
    /// Only when the request asks: Claude, on every host OpenRouter routes it to. The system
    /// prompt carries a cache breakpoint and the request OpenRouter's top-level
    /// `cache_control: {type: "ephemeral"}`, which caches up to the last block (D-215). The
    /// session's warm-up writes the prompt and tools (1.25 × the input price), the first answer
    /// reads them, and each answer reads the one before it (0.1 ×): Claude Haiku 5.5 with
    /// BoardAI's prompt, $0.00058 → $0.00007–0.00021 a request; the first token is unchanged
    /// (~720 ms, 2026-10-16).
    OnRequest,
}

/// What a preset sets for one role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleSettings {
    pub effort: Effort,
    pub max_tokens: u32,
}

impl Preset {
    #[must_use]
    pub fn route(&self) -> Vec<ProviderSlug> {
        self.route
            .iter()
            .filter_map(|slug| ProviderSlug::parse(slug))
            .collect()
    }

    #[must_use]
    pub const fn for_role(&self, role: LlmRole) -> RoleSettings {
        match role {
            LlmRole::Talker => self.talker,
            LlmRole::Scribe => self.scribe,
        }
    }
}

const SCRIBE: RoleSettings = RoleSettings {
    effort: Effort::Medium,
    max_tokens: SCRIBE_MAX_TOKENS_DEFAULT,
};

/// A talker with thinking off, for a model whose reasoning is optional.
const THINKING_OFF: RoleSettings = RoleSettings {
    effort: Effort::None,
    max_tokens: OPENROUTER_TALKER_MAX_TOKENS,
};

/// A talker at the least effort a model whose reasoning is mandatory takes (`low`).
const THINKING_LOW: RoleSettings = RoleSettings {
    effort: Effort::Low,
    max_tokens: OPENROUTER_TALKER_MAX_TOKENS,
};

/// Qwen3.8 27B on Cerebras; as the talker with thinking off (`reasoning.effort = "none"`; the
/// model's own default is `xhigh`).
pub const QWEN_3_8_27B: Preset = Preset {
    model: "qwen/qwen3.8-27b",
    route: &["cerebras"],
    talker: THINKING_OFF,
    scribe: SCRIBE,
    prompt_cache: PromptCache::Automatic,
    drawing: measured(9, 11_000),
};

/// gpt-oss-120b on Cerebras. It always reasons (OpenRouter lists its reasoning as mandatory,
/// with efforts high, medium and low), so as the talker it gets the least: `low`.
pub const GPT_OSS_120B: Preset = Preset {
    model: "openai/gpt-oss-120b",
    route: &["cerebras"],
    talker: THINKING_LOW,
    scribe: SCRIBE,
    prompt_cache: PromptCache::Automatic,
    drawing: measured(11, 1_900),
};

/// DeepSeek V4.1 Flash, thinking off (optional; efforts none, low, high, max), on Together:
/// 297–644 ms to the first token with BoardAI's prompt, where BaseTen US (OpenRouter's fastest
/// median of its 35 providers) refused one request in two with 429 from a shared pool (D-216).
pub const DEEPSEEK_V4_1_FLASH: Preset = Preset {
    model: "deepseek/deepseek-v4.1-flash",
    route: &["together"],
    talker: THINKING_OFF,
    scribe: SCRIBE,
    prompt_cache: PromptCache::Automatic,
    drawing: measured(11, 4_111),
};

/// Claude Haiku 5.5, thinking off (optional), with the prompt cached on request (D-215), on
/// Claude Platform on AWS: as fast as Vertex US with BoardAI's prompt (~720 ms), and Vertex
/// wrote the cache on every request without ever reading it (D-216).
pub const CLAUDE_HAIKU_5_5: Preset = Preset {
    model: "anthropic/claude-haiku-5.5",
    route: &["claude-on-aws"],
    talker: THINKING_OFF,
    scribe: SCRIBE,
    prompt_cache: PromptCache::OnRequest,
    drawing: measured(5, 12_830),
};

/// GPT-6 Luna, thinking off (optional, though on by default), on Amazon Bedrock US East: 485–736
/// ms to the first token with BoardAI's prompt against 754–1,192 on OpenAI's fast tier and
/// 854–1,557 on OpenAI's standard one, at about half the fast tier's price (D-216).
pub const GPT_6_LUNA: Preset = Preset {
    model: "openai/gpt-6-luna",
    route: &["amazon-bedrock/us-east-1"],
    talker: THINKING_OFF,
    scribe: SCRIBE,
    prompt_cache: PromptCache::Automatic,
    drawing: measured(7, 1_725),
};

/// Gemini 3.8 Flash. It always reasons (efforts low, medium and high), so as the talker it
/// gets `low`; on Google AI Studio, as Vertex US took ~8.7 s to the first token with
/// BoardAI's prompt (AI Studio 1.7–2.6 s, D-216).
pub const GEMINI_3_8_FLASH: Preset = Preset {
    model: "google/gemini-3.8-flash",
    route: &["google-ai-studio"],
    talker: THINKING_LOW,
    scribe: SCRIBE,
    prompt_cache: PromptCache::Automatic,
    drawing: measured(12, 3_938),
};

/// The presets, in the order the model picker shows them: the talker and scribe presets, then
/// the measured models (D-189, D-216), fastest to the first token first (`converse`, HLD §11:
/// 374, 520, 788 and 1,733 ms p50).
pub const PRESETS: [Preset; 6] = [
    GPT_OSS_120B,
    QWEN_3_8_27B,
    DEEPSEEK_V4_1_FLASH,
    GPT_6_LUNA,
    CLAUDE_HAIKU_5_5,
    GEMINI_3_8_FLASH,
];

/// The talker picking OpenRouter starts with (D-152; the owner's call).
pub const TALKER_PRESET: Preset = GPT_OSS_120B;

/// The scribe that goes with an OpenRouter talker: the stronger model of the two for digests
/// and memory (intelligence index 33.7 against 11.6 in OpenRouter's model list, 2026-10), on
/// the same fast route, which keeps quitting quick.
pub const SCRIBE_PRESET: Preset = QWEN_3_8_27B;

/// The preset for `model`, if it is one.
#[must_use]
pub fn preset(model: &str) -> Option<Preset> {
    PRESETS.into_iter().find(|preset| preset.model == model)
}

/// How `model` draws as the talker, judged from its preset's measure (D-219); a model that
/// isn't a preset was never measured.
#[must_use]
pub fn drawing(model: &str) -> Drawing {
    Drawing::judge(
        preset(model)
            .and_then(|preset| preset.drawing)
            .map(DrawingMeasured::to_setup),
    )
}

/// How `model`'s prompt is cached: its preset's way, else left to the provider (a model picked
/// from the list gets what OpenRouter does by default).
#[must_use]
pub fn prompt_cache(model: &str) -> PromptCache {
    preset(model).map_or(PromptCache::Automatic, |preset| preset.prompt_cache)
}

/// `reasoning.effort` as OpenRouter documents it: `none` turns thinking off; the other levels
/// pass through (OpenRouter maps them per model).
#[must_use]
pub const fn reasoning_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::None => "none",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::ExtraHigh => "xhigh",
        Effort::Max => "max",
    }
}

/// A route from settings: at most [`OPENROUTER_ROUTE_MAX`] slugs, each valid; the error names
/// the first invalid one by position, never by its text.
pub fn parse_route(slugs: &[String]) -> Result<Vec<ProviderSlug>, RouteError> {
    if slugs.len() > OPENROUTER_ROUTE_MAX {
        return Err(RouteError::TooLong);
    }
    slugs
        .iter()
        .enumerate()
        .map(|(index, slug)| ProviderSlug::parse(slug).ok_or(RouteError::BadSlug { index }))
        .collect()
}

/// Why a route was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteError {
    TooLong,
    BadSlug { index: usize },
}

#[cfg(test)]
mod tests {
    use super::{
        CLAUDE_HAIKU_5_5, PRESETS, PromptCache, ProviderSlug, RouteError, SCRIBE_PRESET,
        TALKER_PRESET, drawing, is_openrouter, parse_route, preset, prompt_cache,
    };
    use crate::limits::{OPENROUTER_ROUTE_MAX, OPENROUTER_SLUG_MAX_BYTES};
    use boardai_core::setup::Drawing;

    #[test]
    fn openrouter_is_recognized_by_its_host_only() {
        assert!(is_openrouter("https://openrouter.ai/api/v1"));
        assert!(is_openrouter("https://openrouter.ai/api/v1/"));
        assert!(!is_openrouter("https://openrouter.ai.example.com/api/v1"));
        assert!(!is_openrouter("http://localhost:11434/v1"));
        assert!(!is_openrouter("not a url"));
    }

    #[test]
    fn slugs_are_lowercase_words_and_bounded() {
        assert!(ProviderSlug::parse("cerebras").is_some());
        assert!(ProviderSlug::parse("deepinfra/turbo").is_some());
        assert!(ProviderSlug::parse("google-vertex/global").is_some());
        for bad in [
            "",
            "Cerebras",
            "a b",
            "/cerebras",
            "cerebras/",
            "sk-or-v1-…",
        ] {
            assert!(ProviderSlug::parse(bad).is_none(), "{bad}");
        }
        let long = "c".repeat(OPENROUTER_SLUG_MAX_BYTES + 1);
        assert!(ProviderSlug::parse(&long).is_none());
        assert!(ProviderSlug::parse(&long[1..]).is_some());
    }

    #[test]
    fn a_served_by_name_matches_its_slug() {
        let cerebras = ProviderSlug::parse("cerebras/fp16");
        assert_eq!(
            cerebras.as_ref().map(|slug| slug.names("Cerebras")),
            Some(true)
        );
        assert_eq!(
            cerebras.as_ref().map(|slug| slug.names("Groq")),
            Some(false)
        );
        let bedrock = ProviderSlug::parse("amazon-bedrock");
        assert_eq!(bedrock.map(|slug| slug.names("Amazon Bedrock")), Some(true));
    }

    /// OpenRouter's names for its providers, from the endpoint lists (2026-10-16): the strict
    /// routes to Vertex and Claude Platform on AWS warned on every answer before.
    #[test]
    fn served_by_names_that_spell_the_slug_otherwise_still_match() {
        let names = |slug: &str, shown: &str| {
            ProviderSlug::parse(slug).is_some_and(|slug| slug.names(shown))
        };
        assert!(names("google-vertex/us", "Google"));
        assert!(names("claude-on-aws", "Claude Platform on AWS"));
        assert!(names("atlas-cloud/fp8", "AtlasCloud"));
        assert!(names("inference-net/fp8", "InferenceNet"));
        assert!(names("google-ai-studio", "Google AI Studio"));
        assert!(names("mancer/fp8", "Mancer 2"));
        assert!(
            !names("google-ai-studio", "Google"),
            "Vertex served an AI Studio route"
        );
        assert!(!names("google-vertex", "Google AI Studio"));
        assert!(!names("anthropic", "Claude Platform on AWS"));
        assert!(!names("cerebras", ""));
    }

    #[test]
    fn routes_are_bounded_and_name_the_bad_slug() {
        let good = vec!["cerebras".to_owned(), "groq".to_owned()];
        assert_eq!(parse_route(&good).map(|route| route.len()), Ok(2));
        let bad = vec!["cerebras".to_owned(), "Groq".to_owned()];
        assert_eq!(parse_route(&bad), Err(RouteError::BadSlug { index: 1 }));
        let long = vec!["cerebras".to_owned(); OPENROUTER_ROUTE_MAX + 1];
        assert_eq!(parse_route(&long), Err(RouteError::TooLong));
    }

    #[test]
    fn the_presets_have_valid_routes_and_are_found_by_model() {
        for preset in PRESETS {
            assert_eq!(preset.route().len(), preset.route.len(), "{}", preset.model);
            assert!(!preset.route.is_empty(), "{} has no route", preset.model);
            assert_eq!(super::preset(preset.model), Some(preset));
        }
        assert!(PRESETS.contains(&TALKER_PRESET));
        assert!(PRESETS.contains(&SCRIBE_PRESET));
        assert_eq!(preset("anthropic/claude-opus-5.5"), None);
    }

    /// D-219, with HLD §11's numbers: gpt-oss-120b, DeepSeek and Gemini drew in at least 10 of
    /// 12 answers; Qwen (9), Luna (7) and Haiku (5) didn't. A model that isn't a preset was never
    /// measured.
    #[test]
    fn the_presets_draw_as_measured() {
        let kind = |model: &str| match drawing(model) {
            Drawing::NotMeasured => "not measured",
            Drawing::Checked { .. } => "checked",
            Drawing::Sometimes { .. } => "sometimes",
        };
        let kinds: Vec<&str> = PRESETS.iter().map(|preset| kind(preset.model)).collect();
        assert_eq!(
            kinds,
            [
                "checked",
                "sometimes",
                "checked",
                "sometimes",
                "sometimes",
                "checked"
            ]
        );
        assert_eq!(kind("anthropic/claude-opus-5.5"), "not measured");
        assert!(PRESETS.iter().all(|preset| {
            preset
                .drawing
                .is_some_and(|measured| measured.drew <= measured.answers)
        }));
    }

    #[test]
    fn only_claude_asks_for_its_prompt_to_be_cached() {
        assert_eq!(prompt_cache(CLAUDE_HAIKU_5_5.model), PromptCache::OnRequest);
        for preset in PRESETS {
            let claude = preset.model.starts_with("anthropic/");
            assert_eq!(
                preset.prompt_cache == PromptCache::OnRequest,
                claude,
                "{}",
                preset.model
            );
        }
        assert_eq!(
            prompt_cache("anthropic/claude-opus-5.5"),
            PromptCache::Automatic
        );
    }
}
