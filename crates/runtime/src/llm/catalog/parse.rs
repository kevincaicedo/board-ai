//! OpenRouter's model lists, parsed once where they enter (`GET /models`, and
//! `GET /models/{id}/endpoints` for one model's providers) into the picker's types. A malformed
//! entry is left out and counted; only a body that isn't a list at all fails.

use boardai_core::setup::{ModelInfo, ModelPrice, ModelProvider, ModelReasoning};
use serde::Deserialize;
use serde_json::Value;

use crate::limits::{MODEL_LIST_MODELS_MAX, MODEL_PROVIDERS_MAX, MODEL_TEXT_MAX_BYTES};
use crate::llm::ProviderError;
use crate::llm::error::bounded_detail;

/// Dollars per token × 10¹² = micro-dollars per million tokens: the price's fractional digits
/// read; digits past the 12th are below one unit and dropped.
const PRICE_PLACES: usize = 12;

/// OpenRouter's reasoning efforts, least first.
const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Deserialize)]
struct List {
    data: Vec<Value>,
}

#[derive(Deserialize)]
struct WireModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<Value>,
    #[serde(default)]
    pricing: Option<WirePricing>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
    #[serde(default)]
    reasoning: Option<WireReasoning>,
}

#[derive(Deserialize)]
struct WirePricing {
    #[serde(default)]
    prompt: Option<Value>,
    #[serde(default)]
    completion: Option<Value>,
}

#[derive(Deserialize)]
struct WireReasoning {
    #[serde(default)]
    mandatory: Option<bool>,
    #[serde(default)]
    supported_efforts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Endpoints {
    data: EndpointsData,
}

#[derive(Deserialize)]
struct EndpointsData {
    endpoints: Vec<Value>,
}

#[derive(Deserialize)]
struct WireEndpoint {
    provider_name: String,
    tag: String,
    #[serde(default)]
    context_length: Option<Value>,
    #[serde(default)]
    max_completion_tokens: Option<Value>,
    #[serde(default)]
    pricing: Option<WirePricing>,
    #[serde(default)]
    quantization: Option<String>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
    /// Medians over the last 30 minutes, `{"p50": …, "p75": …, …}`; `null` in the public list.
    #[serde(default)]
    latency_last_30m: Option<Value>,
    #[serde(default)]
    throughput_last_30m: Option<Value>,
}

/// The tool-capable models of a `/models` answer, in its order, at most
/// [`MODEL_LIST_MODELS_MAX`], with how many were left out (past the bound, malformed, or with
/// an id or name too long to keep). Models without tools are not left out: BoardAI can't use
/// them, so they were never candidates.
pub(super) fn models(body: &[u8]) -> Result<(Vec<ModelInfo>, u32), ProviderError> {
    let list: List = serde_json::from_slice(body).map_err(|error| bad("model list", &error))?;
    let mut models = Vec::new();
    let mut left_out = 0_u32;
    for entry in list.data {
        let Ok(wire) = serde_json::from_value::<WireModel>(entry) else {
            left_out = left_out.saturating_add(1);
            continue;
        };
        let parameters = wire.supported_parameters.as_deref().unwrap_or_default();
        if !parameters.iter().any(|parameter| parameter == "tools") {
            continue;
        }
        match model(&wire, parameters) {
            Some(model) if models.len() < MODEL_LIST_MODELS_MAX => models.push(model),
            Some(_) | None => left_out = left_out.saturating_add(1),
        }
    }
    Ok((models, left_out))
}

fn model(wire: &WireModel, parameters: &[String]) -> Option<ModelInfo> {
    let id = kept(&wire.id)?;
    let name = match wire.name.as_deref() {
        Some(name) => kept(name)?,
        None => id.clone(),
    };
    Some(ModelInfo {
        id,
        name,
        context_tokens: tokens(wire.context_length.as_ref()),
        price: wire.pricing.as_ref().and_then(price),
        reasoning: reasoning(wire.reasoning.as_ref(), parameters),
        preset: false,
        route: Vec::new(),
    })
}

/// `mandatory` reasoning means the model rejects `effort: "none"`, so it gets the least effort
/// it lists; a model that takes the `reasoning` parameter otherwise can turn thinking off.
fn reasoning(wire: Option<&WireReasoning>, parameters: &[String]) -> ModelReasoning {
    if let Some(wire) = wire
        && wire.mandatory == Some(true)
    {
        let listed = wire.supported_efforts.as_deref().unwrap_or_default();
        let least = EFFORTS
            .into_iter()
            .find(|effort| listed.iter().any(|listed| listed == effort))
            .unwrap_or("low");
        return ModelReasoning::Always {
            least_effort: least.to_owned(),
        };
    }
    if wire.is_some() || parameters.iter().any(|parameter| parameter == "reasoning") {
        ModelReasoning::Optional
    } else {
        ModelReasoning::Never
    }
}

/// One model's providers, at most [`MODEL_PROVIDERS_MAX`], with how many were left out.
pub(super) fn providers(body: &[u8]) -> Result<(Vec<ModelProvider>, u32), ProviderError> {
    let answer: Endpoints =
        serde_json::from_slice(body).map_err(|error| bad("provider list", &error))?;
    let mut providers = Vec::new();
    let mut left_out = 0_u32;
    for entry in answer.data.endpoints {
        let provider = serde_json::from_value::<WireEndpoint>(entry)
            .ok()
            .and_then(|wire| provider(&wire));
        match provider {
            Some(provider) if providers.len() < MODEL_PROVIDERS_MAX => providers.push(provider),
            Some(_) | None => left_out = left_out.saturating_add(1),
        }
    }
    Ok((providers, left_out))
}

fn provider(wire: &WireEndpoint) -> Option<ModelProvider> {
    let parameters = wire.supported_parameters.as_deref().unwrap_or_default();
    Some(ModelProvider {
        name: kept(&wire.provider_name)?,
        slug: kept(&wire.tag)?,
        context_tokens: tokens(wire.context_length.as_ref()),
        max_output_tokens: tokens(wire.max_completion_tokens.as_ref()),
        price: wire.pricing.as_ref().and_then(price),
        quantization: wire
            .quantization
            .as_deref()
            .filter(|quantization| *quantization != "unknown")
            .and_then(kept),
        tools: parameters.iter().any(|parameter| parameter == "tools"),
        latency_ms_p50: median(wire.latency_last_30m.as_ref()),
        throughput_tokens_per_second_p50: median(wire.throughput_last_30m.as_ref()),
    })
}

/// The `p50` of a percentile object, rounded to a whole number. Anything else (`null`, a
/// missing or negative median, one past `u32`) is unknown: a speed is shown only when OpenRouter
/// gives one, never guessed.
fn median(value: Option<&Value>) -> Option<u32> {
    let median = value?.get("p50")?.as_f64()?.round();
    if !(0.0..=f64::from(u32::MAX)).contains(&median) {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a whole number in u32's range, checked just above"
    )]
    let whole = median as u32;
    Some(whole)
}

/// Text from a list, kept only when it is short and printable.
fn kept(text: &str) -> Option<String> {
    let text = text.trim();
    let usable = !text.is_empty()
        && text.len() <= MODEL_TEXT_MAX_BYTES
        && !text.chars().any(char::is_control);
    usable.then(|| text.to_owned())
}

/// A token count that fits a `u32`; anything else (absent, negative, fractional) is unknown.
fn tokens(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
}

fn price(wire: &WirePricing) -> Option<ModelPrice> {
    Some(ModelPrice {
        input_micro_usd_per_million_tokens: per_token(wire.prompt.as_ref()?)?,
        output_micro_usd_per_million_tokens: per_token(wire.completion.as_ref()?)?,
    })
}

/// US dollars per token, as OpenRouter writes them (`"0.00000024"`), in millionths of a dollar
/// per million tokens (`240000`), read exactly from the decimal digits. A negative price (the
/// router's "it depends", `"-1"`) or anything not a plain decimal is unknown.
fn per_token(value: &Value) -> Option<u64> {
    let text = value.as_str()?.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || whole.len() > 6 || !digits(whole) || !digits(fraction) {
        return None;
    }
    let mut scaled: u64 = whole.parse().ok()?;
    for place in 0..PRICE_PLACES {
        let digit = fraction.as_bytes().get(place).map_or(0, |byte| byte - b'0');
        scaled = scaled.checked_mul(10)?.checked_add(u64::from(digit))?;
    }
    Some(scaled)
}

fn bad(what: &str, error: &serde_json::Error) -> ProviderError {
    ProviderError::BadOutput {
        detail: bounded_detail(&format!("OpenRouter's {what}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::setup::ModelReasoning;
    use serde_json::json;

    use super::{median, models, per_token, providers};
    use crate::limits::{MODEL_LIST_MODELS_MAX, MODEL_TEXT_MAX_BYTES};

    #[test]
    fn prices_are_read_exactly_from_their_digits() {
        assert_eq!(per_token(&json!("0.00000024")), Some(240_000));
        assert_eq!(per_token(&json!("0.0000022")), Some(2_200_000));
        assert_eq!(per_token(&json!("0")), Some(0));
        assert_eq!(per_token(&json!("1.5")), Some(1_500_000_000_000));
        assert_eq!(per_token(&json!("0.0000000000001")), Some(0));
        for unknown in [
            json!("-1"),
            json!("1e-6"),
            json!(""),
            json!(".5"),
            json!(0.1),
        ] {
            assert_eq!(per_token(&unknown), None, "{unknown}");
        }
    }

    #[test]
    fn only_tool_models_are_kept_and_bad_entries_are_counted() {
        let long = "m".repeat(MODEL_TEXT_MAX_BYTES + 1);
        let body = json!({"data": [
            {"id": "a/tools", "name": "A", "context_length": 8192,
             "pricing": {"prompt": "0.000001", "completion": "0.000002"},
             "supported_parameters": ["tools", "reasoning"]},
            {"id": "b/no-tools", "supported_parameters": ["temperature"]},
            {"id": long, "supported_parameters": ["tools"]},
            {"name": "no id", "supported_parameters": ["tools"]},
            {"id": "c/auto", "pricing": {"prompt": "-1", "completion": "-1"},
             "supported_parameters": ["tools"]},
        ]});
        let (listed, left_out) = models(body.to_string().as_bytes()).expect("a list");
        let ids: Vec<&str> = listed.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["a/tools", "c/auto"]);
        assert_eq!(left_out, 2);
        assert_eq!(listed[0].context_tokens, Some(8_192));
        assert_eq!(
            listed[0]
                .price
                .map(|price| price.output_micro_usd_per_million_tokens),
            Some(2_000_000)
        );
        assert_eq!(listed[0].reasoning, ModelReasoning::Optional);
        assert_eq!(listed[1].name, "c/auto");
        assert_eq!(listed[1].price, None);
        assert_eq!(listed[1].reasoning, ModelReasoning::Never);
    }

    #[test]
    fn mandatory_reasoning_gets_the_least_listed_effort() {
        let body = json!({"data": [
            {"id": "o/oss", "supported_parameters": ["tools", "reasoning"],
             "reasoning": {"mandatory": true, "supported_efforts": ["high", "medium", "low"]}},
            {"id": "q/qwen", "supported_parameters": ["tools", "reasoning"],
             "reasoning": {"mandatory": false, "supported_efforts": ["xhigh", "none"]}},
            {"id": "x/odd", "supported_parameters": ["tools"],
             "reasoning": {"mandatory": true, "supported_efforts": ["turbo"]}},
        ]});
        let (listed, _) = models(body.to_string().as_bytes()).expect("a list");
        let low = ModelReasoning::Always {
            least_effort: "low".to_owned(),
        };
        assert_eq!(listed[0].reasoning, low);
        assert_eq!(listed[1].reasoning, ModelReasoning::Optional);
        assert_eq!(listed[2].reasoning, low);
    }

    #[test]
    fn the_list_is_bounded() {
        let entries: Vec<_> = (0..MODEL_LIST_MODELS_MAX + 3)
            .map(|index| json!({"id": format!("m/{index}"), "supported_parameters": ["tools"]}))
            .collect();
        let body = json!({ "data": entries });
        let (listed, left_out) = models(body.to_string().as_bytes()).expect("a list");
        assert_eq!(listed.len(), MODEL_LIST_MODELS_MAX);
        assert_eq!(left_out, 3);
    }

    #[test]
    fn speeds_are_read_only_where_openrouter_gives_them() {
        assert_eq!(median(Some(&json!({"p50": 287.4, "p90": 900}))), Some(287));
        assert_eq!(median(Some(&json!({"p50": 795}))), Some(795));
        for unknown in [
            json!(null),
            json!({}),
            json!({"p90": 12}),
            json!({"p50": -3}),
            json!({"p50": "fast"}),
            json!({"p50": 1e12}),
            json!(42),
        ] {
            assert_eq!(median(Some(&unknown)), None, "{unknown}");
        }
        assert_eq!(median(None), None);
        let body = json!({"data": {"endpoints": [
            {"provider_name": "Cerebras", "tag": "cerebras/fp16",
             "supported_parameters": ["tools"],
             "latency_last_30m": {"p50": 290.0, "p75": 410.0},
             "throughput_last_30m": {"p50": 795.2}},
            {"provider_name": "Near AI", "tag": "near-ai/fp8",
             "latency_last_30m": null, "throughput_last_30m": null},
        ]}});
        let (listed, _) = providers(body.to_string().as_bytes()).expect("a list");
        assert_eq!(listed[0].latency_ms_p50, Some(290));
        assert_eq!(listed[0].throughput_tokens_per_second_p50, Some(795));
        assert_eq!(listed[1].latency_ms_p50, None);
        assert_eq!(listed[1].throughput_tokens_per_second_p50, None);
    }

    #[test]
    fn a_body_that_is_not_a_list_is_bad_output() {
        for body in [&b"<html>"[..], br#"{"data": {}}"#, br#"{"models": []}"#] {
            assert!(matches!(
                models(body),
                Err(crate::llm::ProviderError::BadOutput { .. })
            ));
        }
        assert!(providers(br#"{"data": []}"#).is_err());
    }
}
