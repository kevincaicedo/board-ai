import assert from "node:assert/strict";
import { test } from "node:test";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import { OLLAMA_MODELS, RECORDED_MODELS, RECORDED_QWEN_PROVIDERS } from "./fakeModels.ts";
import {
  contextLabel,
  dollars,
  filterRows,
  latencyLabel,
  MODEL_ROWS_SHOWN_MAX,
  modelRows,
  orderNote,
  priceLabel,
  routeLabel,
  servingProvider,
  shownRows,
  sortRows,
  speedColumns,
  speedLabel,
} from "./models.ts";

const NONE = new Map<string, readonly ModelProvider[]>();

function measured(
  name: string,
  slug: string,
  latency: number | null,
  speed: number | null,
): ModelProvider {
  return {
    name,
    slug,
    context_tokens: 32_768,
    max_output_tokens: null,
    price: null,
    quantization: null,
    tools: true,
    latency_ms_p50: latency,
    throughput_tokens_per_second_p50: speed,
  };
}

test("OpenRouter's recorded list gives rows with the presets first and no speeds", () => {
  const rows = modelRows(RECORDED_MODELS, "OpenRouter", NONE);
  assert.deepEqual(
    sortRows(rows).map((row) => row.id),
    [
      "openai/gpt-oss-120b",
      "qwen/qwen3.8-27b",
      "xingchen-agi/xing4.0-29b-a4b:free",
      "unbiased/pareto-26.9",
      "meta/muse-wonder",
    ],
  );
  const oss = rows[0];
  assert.equal(oss?.servedBy, "Cerebras only");
  assert.equal(priceLabel(oss?.price ?? null), "$0.15 / $0.60");
  assert.equal(contextLabel(oss?.contextTokens ?? null), "131K");
  assert.equal(rows[2]?.servedBy, "OpenRouter picks");
  // OpenRouter's public list gives no first token or speed: the columns stay out (D-176).
  assert.equal(speedColumns(rows), false);
  assert.equal(orderNote(rows, "OpenRouter"), "tools only · presets first");
  assert.ok(rows.every((row) => row.latencyMs === null && row.tokensPerSecond === null));
});

test("a preset row shows its route's provider once that list is loaded", () => {
  const providers = new Map([["qwen/qwen3.8-27b", RECORDED_QWEN_PROVIDERS]]);
  const qwen = modelRows(RECORDED_MODELS, "OpenRouter", providers).find(
    (row) => row.id === "qwen/qwen3.8-27b",
  );
  // Cerebras serves it on the preset's route: its price and context, not the list's cheapest.
  assert.equal(qwen?.servedBy, "Cerebras only");
  assert.equal(priceLabel(qwen?.price ?? null), "$0.99 / $1.49");
  assert.equal(contextLabel(qwen?.contextTokens ?? null), "65K");
  assert.equal(qwen?.latencyMs, null, "recorded without a speed, so none is shown");
});

test("speeds show, and order the rows, only where OpenRouter gives them", () => {
  const providers = new Map<string, readonly ModelProvider[]>([
    [
      "unbiased/pareto-26.9",
      [measured("Slow", "slow", 900, 40), measured("Quick", "quick/fp8", 300, 400)],
    ],
    ["meta/muse-wonder", [measured("Mid", "mid", 500, 120)]],
  ]);
  const rows = modelRows(RECORDED_MODELS, "OpenRouter", providers);
  const pareto = rows.find((row) => row.id === "unbiased/pareto-26.9");
  assert.equal(
    pareto?.tokensPerSecond,
    400,
    "with no route, the fastest provider that takes tools",
  );
  assert.equal(pareto?.servedBy, "OpenRouter picks · 2 providers");
  assert.equal(speedColumns(rows), true);
  assert.equal(orderNote(rows, "OpenRouter"), "tools only · fastest first");
  assert.deepEqual(
    sortRows(rows).map((row) => row.id),
    [
      "openai/gpt-oss-120b",
      "qwen/qwen3.8-27b",
      "unbiased/pareto-26.9",
      "meta/muse-wonder",
      "xingchen-agi/xing4.0-29b-a4b:free",
    ],
    "presets, then known speed, fastest first, then the rest in the app's order",
  );
  assert.equal(latencyLabel(pareto?.latencyMs ?? null), "0.30 s");
  assert.equal(speedLabel(pareto?.tokensPerSecond ?? null), "400 tok/s");
  assert.equal(latencyLabel(null), "–");
  assert.equal(speedLabel(null), "–");
});

test("a routed model's numbers come from its route, never another provider", () => {
  const providers = [measured("Fast but elsewhere", "other", 100, 2000)];
  assert.equal(servingProvider(["cerebras"], providers), null);
  assert.equal(
    servingProvider([], [{ ...measured("No tools", "x", 50, 5000), tools: false }]),
    null,
  );
  assert.equal(
    routeLabel(["cerebras", "groq"], RECORDED_QWEN_PROVIDERS),
    "Cerebras, then Groq only",
  );
  assert.equal(routeLabel([], []), "OpenRouter picks");
});

test("a search matches every word against the id, name, provider and tags", () => {
  const rows = modelRows(RECORDED_MODELS, "OpenRouter", NONE);
  const ids = (search: string): string[] => filterRows(rows, search).map((row) => row.id);
  assert.deepEqual(ids("qwen"), ["qwen/qwen3.8-27b"]);
  assert.deepEqual(ids("GPT-OSS"), ["openai/gpt-oss-120b"]);
  assert.deepEqual(ids("free"), ["xingchen-agi/xing4.0-29b-a4b:free"]);
  assert.deepEqual(ids("preset cerebras"), ["openai/gpt-oss-120b", "qwen/qwen3.8-27b"]);
  assert.deepEqual(ids("thinks meta"), ["meta/muse-wonder"]);
  assert.equal(ids("tools").length, rows.length, "every OpenRouter row takes tools");
  assert.equal(ids("   ").length, rows.length);
  assert.deepEqual(ids("qwen claude"), []);
});

test("Ollama's rows are local, with no price and the default first", () => {
  const rows = modelRows(OLLAMA_MODELS, "Ollama", NONE);
  assert.deepEqual(
    rows.map((row) => [
      row.id,
      row.servedBy,
      priceLabel(row.price),
      contextLabel(row.contextTokens),
    ]),
    [
      ["gemma4:latest", "This Mac", "–", "–"],
      ["qwen3.5:2b", "This Mac", "–", "–"],
    ],
  );
  assert.equal(orderNote(rows, "Ollama"), "on this Mac · presets first");
});

test("the table draws a bounded number of rows and keeps the chosen one", () => {
  const many = Array.from({ length: MODEL_ROWS_SHOWN_MAX + 5 }, (_, index) => ({
    id: `author/model-${index}`,
    name: `Model ${index}`,
    context_tokens: null,
    price: null,
    reasoning: { kind: "Unknown" as const },
    preset: false,
    route: [],
  }));
  const rows = modelRows(many, "OpenRouter", NONE);
  const last = `author/model-${MODEL_ROWS_SHOWN_MAX + 4}`;
  const shown = shownRows(rows, "", last);
  assert.equal(shown.rows.length, MODEL_ROWS_SHOWN_MAX + 1);
  assert.equal(shown.rows.at(-1)?.id, last);
  assert.equal(shown.hidden, 4);
  const narrowed = shownRows(rows, "model-1", null);
  assert.equal(narrowed.hidden, 0);
  assert.ok(narrowed.rows.every((row) => row.id.includes("model-1")));
});

test("prices and context read as the canvas writes them", () => {
  assert.equal(dollars(0), "$0");
  assert.equal(dollars(9_000), "$0.009");
  assert.equal(dollars(2_200_000), "$2.20");
  assert.equal(
    priceLabel({ input_micro_usd_per_million_tokens: 0, output_micro_usd_per_million_tokens: 0 }),
    "free",
  );
  assert.equal(priceLabel(null), "–");
  assert.equal(contextLabel(65_536), "65K");
  assert.equal(contextLabel(1_000_000), "1M");
  assert.equal(contextLabel(1_048_576), "1.04M");
  assert.equal(contextLabel(512), "512");
  assert.equal(contextLabel(null), "–");
});
