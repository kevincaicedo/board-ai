//! Whether a preset still fits what OpenRouter lists for its model (D-217). A preset was
//! measured with a thinking setting and a strict route (D-152); OpenRouter's lists change, and
//! a preset that no longer fits is refused at the first request (a mandatory-reasoning model
//! rejects `none`; a route whose provider dropped tools has no endpoint that takes every
//! parameter). The catalog checks each preset when a list arrives and logs what no longer fits,
//! so a stale preset is found from the logs before the owner hears it fail.

use boardai_core::setup::{ModelInfo, ModelProvider, ModelReasoning};

use super::talker_effort;
use crate::llm::Effort;
use crate::llm::openrouter::Preset;

/// Why a preset no longer fits its listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetMisfit {
    /// The model takes no reasoning setting, and a strict route requires every parameter sent.
    NoReasoning,
    /// The preset turns thinking off, but the model always reasons.
    ThinkingOffRefused,
    /// The preset thinks less than the least the model takes.
    BelowLeastEffort { least: Effort },
    /// The route names a provider the model's list doesn't.
    RouteNotListed { slug: String },
    /// The route's provider serves the model without tools.
    RouteWithoutTools { slug: String },
}

/// What in `preset` its model's listing refuses, if anything.
#[must_use]
pub fn listing_misfit(preset: &Preset, listed: &ModelInfo) -> Option<PresetMisfit> {
    let effort = preset.talker.effort;
    match &listed.reasoning {
        ModelReasoning::Never => Some(PresetMisfit::NoReasoning),
        ModelReasoning::Unknown | ModelReasoning::Optional => None,
        ModelReasoning::Always { .. } => {
            let least = talker_effort(&listed.reasoning);
            if effort == Effort::None {
                Some(PresetMisfit::ThinkingOffRefused)
            } else if rank(effort) < rank(least) {
                Some(PresetMisfit::BelowLeastEffort { least })
            } else {
                None
            }
        }
    }
}

/// The first provider of `preset`'s route that its model's provider list doesn't have, or has
/// without tools. A route slug names an endpoint (`google-vertex/us`) or every endpoint of a
/// provider (`cerebras` for `cerebras/fp16`).
#[must_use]
pub fn route_misfit(preset: &Preset, providers: &[ModelProvider]) -> Option<PresetMisfit> {
    for slug in preset.route {
        let serving: Vec<&ModelProvider> = providers
            .iter()
            .filter(|provider| {
                provider.slug == *slug || provider.slug.split('/').next() == Some(slug)
            })
            .collect();
        if serving.is_empty() {
            return Some(PresetMisfit::RouteNotListed {
                slug: (*slug).to_owned(),
            });
        }
        if !serving.iter().any(|provider| provider.tools) {
            return Some(PresetMisfit::RouteWithoutTools {
                slug: (*slug).to_owned(),
            });
        }
    }
    None
}

/// Least thinking first.
const fn rank(effort: Effort) -> u8 {
    match effort {
        Effort::None => 0,
        Effort::Low => 1,
        Effort::Medium => 2,
        Effort::High => 3,
        Effort::ExtraHigh => 4,
        Effort::Max => 5,
    }
}
