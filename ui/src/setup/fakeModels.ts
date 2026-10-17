// Model lists for the fake app, copied from what OpenRouter answered on 2026-10-16
// (crates/runtime/tests/fixtures/openrouter/: models.json, models_measured.json,
// endpoints_qwen3.8-27b.json, endpoints_deepseek-v4.1-flash.json), as the app parses them, with
// each preset's drawing as spike S5 measured it (HLD §11, D-219). Prices are integer
// micro-dollars per million tokens. OpenRouter's public list gave no first-token time or speed
// for any provider (Qwen's here); asked with the key it did (DeepSeek's, D-218). None is made up.

import type { Drawing } from "../protocol/generated/Drawing.ts";
import type { ModelInfo } from "../protocol/generated/ModelInfo.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { ModelQuery } from "../protocol/generated/ModelQuery.ts";

function price(input: number, output: number) {
  return { input_micro_usd_per_million_tokens: input, output_micro_usd_per_million_tokens: output };
}

/** A preset's drawing as the core judged its 2026-10-16 measure (D-219). */
function drew(kind: "Checked" | "Sometimes", count: number, firstStrokeMs: number): Drawing {
  const measure = {
    drew: count,
    answers: 12,
    first_stroke_ms_p50: firstStrokeMs,
    measured_on: "2026-10-16",
  };
  return { kind, measure };
}

/** gpt-oss-120b's, which the fake settings page shows for its talker. */
export const GPT_OSS_DRAWING: Drawing = drew("Checked", 11, 1_900);

const NOT_MEASURED: Drawing = { kind: "NotMeasured" };

/** OpenRouter's tool models, the six presets first in their order, as `setup_models` lists them. */
export const RECORDED_MODELS: readonly ModelInfo[] = [
  {
    id: "openai/gpt-oss-120b",
    name: "OpenAI: gpt-oss-120b",
    context_tokens: 131_072,
    price: price(150_000, 600_000),
    reasoning: { kind: "Always", least_effort: "low" },
    preset: true,
    route: ["cerebras"],
    drawing: GPT_OSS_DRAWING,
  },
  {
    id: "qwen/qwen3.8-27b",
    name: "Qwen: Qwen3.8 27B",
    context_tokens: 1_000_000,
    price: price(240_000, 2_200_000),
    reasoning: { kind: "Optional" },
    preset: true,
    route: ["cerebras"],
    drawing: drew("Sometimes", 9, 11_000),
  },
  {
    id: "deepseek/deepseek-v4.1-flash",
    name: "DeepSeek: DeepSeek V4.1 Flash",
    context_tokens: 1_048_576,
    price: price(150_000, 600_000),
    reasoning: { kind: "Optional" },
    preset: true,
    route: ["together"],
    drawing: drew("Checked", 11, 4_111),
  },
  {
    id: "openai/gpt-6-luna",
    name: "OpenAI: GPT-6 Luna",
    context_tokens: 1_050_000,
    price: price(50_000, 250_000),
    reasoning: { kind: "Optional" },
    preset: true,
    route: ["amazon-bedrock/us-east-1"],
    drawing: drew("Sometimes", 7, 1_725),
  },
  {
    id: "anthropic/claude-haiku-5.5",
    name: "Anthropic: Claude Haiku 5.5",
    context_tokens: 1_000_000,
    price: price(100_000, 500_000),
    reasoning: { kind: "Optional" },
    preset: true,
    route: ["claude-on-aws"],
    drawing: drew("Sometimes", 5, 12_830),
  },
  {
    id: "google/gemini-3.8-flash",
    name: "Google: Gemini 3.8 Flash",
    context_tokens: 1_048_576,
    price: price(375_000, 1_875_000),
    reasoning: { kind: "Always", least_effort: "low" },
    preset: true,
    route: ["google-ai-studio"],
    drawing: drew("Checked", 12, 3_938),
  },
  {
    id: "xingchen-agi/xing4.0-29b-a4b:free",
    name: "XingChen: Xing4.0 29B A4B (free)",
    context_tokens: 262_144,
    price: price(0, 0),
    reasoning: { kind: "Optional" },
    preset: false,
    route: [],
    drawing: NOT_MEASURED,
  },
  {
    id: "unbiased/pareto-26.9",
    name: "Pareto 26.9",
    context_tokens: 262_144,
    price: price(2_500_000, 7_500_000),
    reasoning: { kind: "Never" },
    preset: false,
    route: [],
    drawing: NOT_MEASURED,
  },
  {
    id: "meta/muse-wonder",
    name: "Meta: Muse Wonder",
    context_tokens: 1_048_576,
    price: price(4_000_000, 20_000_000),
    reasoning: { kind: "Always", least_effort: "minimal" },
    preset: false,
    route: [],
    drawing: NOT_MEASURED,
  },
];

function provider(
  name: string,
  slug: string,
  context: number,
  served: readonly [number, number],
  medians: readonly [number, number] | null = null,
): ModelProvider {
  return {
    name,
    slug,
    context_tokens: context,
    max_output_tokens: null,
    price: price(served[0], served[1]),
    quantization: slug.includes("/") ? (slug.split("/")[1] ?? null) : null,
    tools: true,
    latency_ms_p50: medians === null ? null : medians[0],
    throughput_tokens_per_second_p50: medians === null ? null : medians[1],
  };
}

/** A few of Qwen3.8 27B's twenty providers, Cerebras (its preset route) among them. */
export const RECORDED_QWEN_PROVIDERS: readonly ModelProvider[] = [
  provider("Tenstorrent", "tenstorrent/fp4", 262_144, [20_000, 500_000]),
  provider("Near AI", "near-ai/fp8", 262_144, [40_000, 1_350_000]),
  provider("DeepInfra", "deepinfra/bf16", 262_144, [150_000, 1_875_000]),
  provider("Cerebras", "cerebras/fp16", 65_536, [990_000, 1_490_000]),
];

/**
 * A few of DeepSeek V4.1 Flash's 35 providers, Together (its preset route) among them, from the
 * list asked with the key: OpenRouter's latency and throughput medians (D-218).
 */
export const RECORDED_DEEPSEEK_PROVIDERS: readonly ModelProvider[] = [
  provider("BaseTen", "baseten/us", 1_048_576, [300_000, 1_200_000], [199, 172]),
  provider("Together", "together", 1_048_576, [300_000, 1_200_000], [312, 185]),
  provider("BaseTen", "baseten", 1_048_576, [300_000, 1_200_000], [374, 194]),
  provider("Modal", "modal", 1_048_576, [300_000, 1_200_000], [403, 167]),
];

/** Ollama's models on the owner's Mac, the default first. */
export const OLLAMA_MODELS: readonly ModelInfo[] = ["gemma4:latest", "qwen3.5:2b"].map((id) => ({
  id,
  name: id,
  context_tokens: null,
  price: null,
  reasoning: { kind: "Unknown" },
  preset: id === "gemma4:latest",
  route: [],
  drawing: NOT_MEASURED,
}));

/** What the fake app answers each query with. */
export function recordedListing(query: ModelQuery): ModelListing {
  switch (query.kind) {
    case "OpenRouter":
      return { kind: "OpenRouter", models: [...RECORDED_MODELS], left_out: 0 };
    case "Ollama":
      return { kind: "Ollama", models: [...OLLAMA_MODELS] };
    case "Providers":
      return {
        kind: "Providers",
        model: query.model,
        providers: recordedProviders(query.model),
        left_out: 0,
      };
  }
}

function recordedProviders(model: string): ModelProvider[] {
  switch (model) {
    case "qwen/qwen3.8-27b":
      return [...RECORDED_QWEN_PROVIDERS];
    case "deepseek/deepseek-v4.1-flash":
      return [...RECORDED_DEEPSEEK_PROVIDERS];
    default:
      return [];
  }
}
