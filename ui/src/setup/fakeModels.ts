// Model lists for the fake app, copied from what OpenRouter answered on 2026-10-16
// (crates/runtime/tests/fixtures/openrouter/: models.json, endpoints_qwen3.8-27b.json), as the
// app parses them. Prices are integer micro-dollars per million tokens. OpenRouter's public list
// gave no first-token time or speed for any provider, so none is made up here either.

import type { ModelInfo } from "../protocol/generated/ModelInfo.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { ModelQuery } from "../protocol/generated/ModelQuery.ts";

function price(input: number, output: number) {
  return { input_micro_usd_per_million_tokens: input, output_micro_usd_per_million_tokens: output };
}

/** OpenRouter's tool models, presets first, as `setup_models` lists them. */
export const RECORDED_MODELS: readonly ModelInfo[] = [
  {
    id: "openai/gpt-oss-120b",
    name: "OpenAI: gpt-oss-120b",
    context_tokens: 131_072,
    price: price(150_000, 600_000),
    reasoning: { kind: "Always", least_effort: "low" },
    preset: true,
    route: ["cerebras"],
  },
  {
    id: "qwen/qwen3.8-27b",
    name: "Qwen: Qwen3.8 27B",
    context_tokens: 1_000_000,
    price: price(240_000, 2_200_000),
    reasoning: { kind: "Optional" },
    preset: true,
    route: ["cerebras"],
  },
  {
    id: "xingchen-agi/xing4.0-29b-a4b:free",
    name: "XingChen: Xing4.0 29B A4B (free)",
    context_tokens: 262_144,
    price: price(0, 0),
    reasoning: { kind: "Optional" },
    preset: false,
    route: [],
  },
  {
    id: "unbiased/pareto-26.9",
    name: "Pareto 26.9",
    context_tokens: 262_144,
    price: price(2_500_000, 7_500_000),
    reasoning: { kind: "Never" },
    preset: false,
    route: [],
  },
  {
    id: "meta/muse-wonder",
    name: "Meta: Muse Wonder",
    context_tokens: 1_048_576,
    price: price(4_000_000, 20_000_000),
    reasoning: { kind: "Always", least_effort: "minimal" },
    preset: false,
    route: [],
  },
];

function provider(
  name: string,
  slug: string,
  context: number,
  served: readonly [number, number],
): ModelProvider {
  return {
    name,
    slug,
    context_tokens: context,
    max_output_tokens: null,
    price: price(served[0], served[1]),
    quantization: slug.includes("/") ? (slug.split("/")[1] ?? null) : null,
    tools: true,
    latency_ms_p50: null,
    throughput_tokens_per_second_p50: null,
  };
}

/** A few of Qwen3.8 27B's twenty providers, Cerebras (its preset route) among them. */
export const RECORDED_QWEN_PROVIDERS: readonly ModelProvider[] = [
  provider("Tenstorrent", "tenstorrent/fp4", 262_144, [20_000, 500_000]),
  provider("Near AI", "near-ai/fp8", 262_144, [40_000, 1_350_000]),
  provider("DeepInfra", "deepinfra/bf16", 262_144, [150_000, 1_875_000]),
  provider("Cerebras", "cerebras/fp16", 65_536, [990_000, 1_490_000]),
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
        providers: query.model === "qwen/qwen3.8-27b" ? [...RECORDED_QWEN_PROVIDERS] : [],
        left_out: 0,
      };
  }
}
