//! What each Claude model accepts, from the claude-api reference (cached 2026-10-06).
//!
//! An unknown model gets none of these features: a request without them works on every model,
//! while sending one a model doesn't support is a 400.

/// Request features that depend on the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelTraits {
    /// `thinking: {type: "adaptive"}`.
    pub adaptive_thinking: bool,
    /// `output_config.effort`.
    pub effort: bool,
    /// `role: "system"` messages after the first user turn, with no beta header.
    pub mid_conversation_system: bool,
    /// `fallbacks: "default"` under `server-side-fallback-2026-07-01`.
    pub server_side_fallback: bool,
}

/// Models with adaptive thinking and effort (4.6 and later; Haiku from 5.5).
const ADAPTIVE: &[&str] = &[
    "claude-fable-5-1",
    "claude-fable-5",
    "claude-mythos-5-1",
    "claude-mythos-5",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-5-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-haiku-5-5",
];

/// Models that take mid-conversation system messages. Claude Sonnet 5 is left out: the docs
/// conflict on it, and a 400 there is worse than folding the text into the user turn.
const MID_CONVERSATION_SYSTEM: &[&str] = &[
    "claude-fable-5-1",
    "claude-fable-5",
    "claude-mythos-5-1",
    "claude-mythos-5",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

/// Models whose refusals a server-side fallback can rescue. Claude Haiku 5.5 has no
/// server-side fallback.
const SERVER_SIDE_FALLBACK: &[&str] = &[
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];

/// Exact ids only: `claude-opus-5` is a prefix of `claude-opus-5-5`, and ids never carry a
/// date suffix.
pub(crate) fn traits(model: &str) -> ModelTraits {
    let adaptive = ADAPTIVE.contains(&model);
    ModelTraits {
        adaptive_thinking: adaptive,
        effort: adaptive,
        mid_conversation_system: MID_CONVERSATION_SYSTEM.contains(&model),
        server_side_fallback: SERVER_SIDE_FALLBACK.contains(&model),
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelTraits, traits};

    #[test]
    fn the_defaults_get_every_feature_they_support() {
        let talker = traits("claude-opus-5-5");
        assert!(talker.adaptive_thinking && talker.effort);
        assert!(talker.mid_conversation_system && talker.server_side_fallback);
        let scribe = traits("claude-haiku-5-5");
        assert!(scribe.adaptive_thinking && scribe.mid_conversation_system);
        assert!(!scribe.server_side_fallback);
    }

    #[test]
    fn unknown_and_older_models_get_none() {
        assert_eq!(traits("claude-haiku-4-5"), ModelTraits::default());
        assert_eq!(traits("claude-opus-5-5-20260101"), ModelTraits::default());
        assert!(!traits("claude-sonnet-5").mid_conversation_system);
    }
}
