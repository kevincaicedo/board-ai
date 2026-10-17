//! OpenRouter (LLD §3.3): the presets BoardAI is measured with, provider routing, and the
//! `reasoning` setting OpenRouter documents for every model (D-152).
//!
//! OpenRouter speaks the OpenAI-compatible protocol, so [`super::openai`] renders its requests;
//! this module holds what differs: a route (`provider.order`) and the `reasoning` object in
//! place of `reasoning_effort`.

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
    /// this one: its display name, lowercased with spaces as `-`, is the slug's first part.
    #[must_use]
    pub fn names(&self, provider_name: &str) -> bool {
        let base = self.0.split('/').next().unwrap_or_default();
        let shown = provider_name.trim().to_ascii_lowercase().replace(' ', "-");
        !base.is_empty() && shown == base
    }
}

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

/// Qwen3.8 27B on Cerebras; as the talker with thinking off (`reasoning.effort = "none"`; the
/// model's own default is `xhigh`).
pub const QWEN_3_8_27B: Preset = Preset {
    model: "qwen/qwen3.8-27b",
    route: &["cerebras"],
    talker: RoleSettings {
        effort: Effort::None,
        max_tokens: OPENROUTER_TALKER_MAX_TOKENS,
    },
    scribe: SCRIBE,
};

/// gpt-oss-120b on Cerebras. It always reasons (OpenRouter lists its reasoning as mandatory,
/// with efforts high, medium and low), so as the talker it gets the least: `low`.
pub const GPT_OSS_120B: Preset = Preset {
    model: "openai/gpt-oss-120b",
    route: &["cerebras"],
    talker: RoleSettings {
        effort: Effort::Low,
        max_tokens: OPENROUTER_TALKER_MAX_TOKENS,
    },
    scribe: SCRIBE,
};

/// The presets, in the order the model picker shows them.
pub const PRESETS: [Preset; 2] = [GPT_OSS_120B, QWEN_3_8_27B];

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
        PRESETS, ProviderSlug, RouteError, SCRIBE_PRESET, TALKER_PRESET, is_openrouter,
        parse_route, preset,
    };
    use crate::limits::{OPENROUTER_ROUTE_MAX, OPENROUTER_SLUG_MAX_BYTES};

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
    fn the_presets_route_to_cerebras_and_are_found_by_model() {
        for preset in PRESETS {
            assert_eq!(preset.route().len(), preset.route.len(), "{}", preset.model);
            assert_eq!(super::preset(preset.model), Some(preset));
        }
        assert!(PRESETS.contains(&TALKER_PRESET));
        assert!(PRESETS.contains(&SCRIBE_PRESET));
        assert_eq!(preset("anthropic/claude-opus-5.5"), None);
    }
}
