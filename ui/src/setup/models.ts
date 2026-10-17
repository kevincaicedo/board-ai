// The settings page's model table (artboard 12, D-153): rows from `setup_models`, a search, the
// order, and how numbers read. Pure, so node --test covers every rule.
//
// Never a made-up number: a row's first token and speed come only from OpenRouter's provider
// list (the provider a preset is routed to, else the fastest that takes tools), and the two
// columns show only while a shown row has one. OpenRouter's public list gives none today, so
// the table leaves them out and its order is presets first, then OpenRouter's (D-176).

import type { ModelInfo } from "../protocol/generated/ModelInfo.ts";
import type { ModelPrice } from "../protocol/generated/ModelPrice.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";

/** Rows drawn at once; the rest are counted and a search finds them. */
export const MODEL_ROWS_SHOWN_MAX = 40;

/** Where a model runs, for a row's second line. */
export type Source = "OpenRouter" | "Ollama";

export interface ModelRow {
  readonly id: string;
  readonly name: string;
  readonly preset: boolean;
  /** "Cerebras only", "OpenRouter picks · 20 providers", "This Mac". */
  readonly servedBy: string;
  /** What a search matches besides the id and name: `tools`, `preset`, `free`, `thinks`. */
  readonly tags: readonly string[];
  readonly latencyMs: number | null;
  readonly tokensPerSecond: number | null;
  readonly price: ModelPrice | null;
  readonly contextTokens: number | null;
  /** Its place in the app's list, which keeps the order stable among equals. */
  readonly listed: number;
}

/** The first part of a provider's route slug: `cerebras/fp16` → `cerebras`. */
export function slugBase(slug: string): string {
  return slug.split("/")[0] ?? slug;
}

/** A route slug as a name: the provider list's own when it is known, else the slug, capitalized. */
export function providerName(slug: string, providers: readonly ModelProvider[]): string {
  const listed = providers.find((provider) => slugBase(provider.slug) === slug);
  if (listed !== undefined) {
    return listed.name;
  }
  return slug.length === 0 ? slug : `${slug[0]?.toUpperCase() ?? ""}${slug.slice(1)}`;
}

/** "Cerebras only" for a strict route (D-152); "OpenRouter picks" with no route. */
export function routeLabel(route: readonly string[], providers: readonly ModelProvider[]): string {
  if (route.length === 0) {
    return "OpenRouter picks";
  }
  return `${route.map((slug) => providerName(slug, providers)).join(", then ")} only`;
}

/**
 * The provider whose numbers a row shows: the first of a preset's route, or, with no route, the
 * fastest one that takes tools (OpenRouter may pick another; the row says it picks).
 */
export function servingProvider(
  route: readonly string[],
  providers: readonly ModelProvider[],
): ModelProvider | null {
  const first = route[0];
  if (first !== undefined) {
    return providers.find((provider) => slugBase(provider.slug) === first) ?? null;
  }
  let fastest: ModelProvider | null = null;
  for (const provider of providers) {
    const speed = provider.throughput_tokens_per_second_p50;
    if (!provider.tools || speed === null) {
      continue;
    }
    if (fastest === null || speed > (fastest.throughput_tokens_per_second_p50 ?? 0)) {
      fastest = provider;
    }
  }
  return fastest;
}

function isFree(price: ModelPrice | null): boolean {
  return (
    price !== null &&
    price.input_micro_usd_per_million_tokens === 0 &&
    price.output_micro_usd_per_million_tokens === 0
  );
}

/**
 * The table's rows for a listing. `providers` holds the provider lists the page has loaded,
 * by model id; a row whose list is loaded shows its serving provider's price and context.
 */
export function modelRows(
  models: readonly ModelInfo[],
  source: Source,
  providers: ReadonlyMap<string, readonly ModelProvider[]>,
): ModelRow[] {
  return models.map((model, listed) => {
    const served = providers.get(model.id) ?? [];
    const serving = source === "OpenRouter" ? servingProvider(model.route, served) : null;
    const tags = [
      ...(source === "OpenRouter" ? ["tools"] : ["local"]),
      ...(model.preset ? ["preset"] : []),
      ...(isFree(model.price) ? ["free"] : []),
      ...(model.reasoning.kind === "Always" ? ["thinks"] : []),
    ];
    const count =
      served.length > 0 && model.route.length === 0 ? ` · ${served.length} providers` : "";
    return {
      id: model.id,
      name: model.name,
      preset: model.preset,
      servedBy: source === "Ollama" ? "This Mac" : `${routeLabel(model.route, served)}${count}`,
      tags,
      latencyMs: serving?.latency_ms_p50 ?? null,
      tokensPerSecond: serving?.throughput_tokens_per_second_p50 ?? null,
      price: serving?.price ?? model.price,
      contextTokens: serving?.context_tokens ?? model.context_tokens,
      listed,
    };
  });
}

/**
 * Rows matching every word of the search, in any order: a word matches the id, the name, who
 * serves it, or a tag. An empty search keeps every row.
 */
export function filterRows(rows: readonly ModelRow[], search: string): ModelRow[] {
  const words = search
    .toLowerCase()
    .split(/\s+/)
    .filter((word) => word.length > 0);
  if (words.length === 0) {
    return [...rows];
  }
  return rows.filter((row) => {
    const haystack = [row.id, row.name, row.servedBy, ...row.tags].join(" ").toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}

/**
 * Fastest first: presets lead (BoardAI measured them, D-152), then rows by their known speed,
 * then the rest in the app's order. With no speed known this is the app's own order.
 */
export function sortRows(rows: readonly ModelRow[]): ModelRow[] {
  return [...rows].sort((left, right) => {
    if (left.preset !== right.preset) {
      return left.preset ? -1 : 1;
    }
    const leftSpeed = left.tokensPerSecond;
    const rightSpeed = right.tokensPerSecond;
    if (leftSpeed !== null && rightSpeed !== null && leftSpeed !== rightSpeed) {
      return rightSpeed - leftSpeed;
    }
    if ((leftSpeed === null) !== (rightSpeed === null)) {
      return leftSpeed === null ? 1 : -1;
    }
    return left.listed - right.listed;
  });
}

/** The first-token and speed columns show only while a shown row has a number for them. */
export function speedColumns(rows: readonly ModelRow[]): boolean {
  return rows.some((row) => row.latencyMs !== null || row.tokensPerSecond !== null);
}

/** What the search bar's note says the order is. */
export function orderNote(rows: readonly ModelRow[], source: Source): string {
  const scope = source === "OpenRouter" ? "tools only" : "on this Mac";
  return `${scope} · ${speedColumns(rows) ? "fastest first" : "presets first"}`;
}

export interface ShownRows {
  readonly rows: readonly ModelRow[];
  /** Rows past the bound, which a search narrows down. */
  readonly hidden: number;
}

/**
 * The rows drawn: filtered, sorted and bounded. The chosen model always shows when it matches
 * the search, even past the bound.
 */
export function shownRows(
  rows: readonly ModelRow[],
  search: string,
  selected: string | null,
): ShownRows {
  const matching = sortRows(filterRows(rows, search));
  const first = matching.slice(0, MODEL_ROWS_SHOWN_MAX);
  const chosen = matching.slice(MODEL_ROWS_SHOWN_MAX).find((row) => row.id === selected);
  return {
    rows: chosen === undefined ? first : [...first, chosen],
    hidden: matching.length - first.length - (chosen === undefined ? 0 : 1),
  };
}

const MICRO_PER_DOLLAR = 1_000_000;

/** Dollars per million tokens: `$0.15`, `$2.20`, `$0.009`, `$0`. */
export function dollars(micro: number): string {
  if (micro === 0) {
    return "$0";
  }
  const value = micro / MICRO_PER_DOLLAR;
  const places = value < 0.01 ? 3 : 2;
  return `$${value.toFixed(places)}`;
}

/** "$0.15 / $0.60", "free", or "–" when the list gives no fixed price. */
export function priceLabel(price: ModelPrice | null): string {
  if (price === null) {
    return "–";
  }
  if (isFree(price)) {
    return "free";
  }
  return `${dollars(price.input_micro_usd_per_million_tokens)} / ${dollars(price.output_micro_usd_per_million_tokens)}`;
}

/** Tokens as the canvas writes them: `131K`, `65K`, `1M`, `1.04M`; "–" unknown. */
export function contextLabel(tokens: number | null): string {
  if (tokens === null) {
    return "–";
  }
  if (tokens >= 1_000_000) {
    const millions = Math.floor(tokens / 10_000) / 100;
    return `${millions}M`;
  }
  if (tokens >= 1_000) {
    return `${Math.floor(tokens / 1_000)}K`;
  }
  return String(tokens);
}

/** "0.29 s"; "–" when OpenRouter gave none. */
export function latencyLabel(latencyMs: number | null): string {
  return latencyMs === null ? "–" : `${(latencyMs / 1_000).toFixed(2)} s`;
}

/** "795 tok/s"; "–" when OpenRouter gave none. */
export function speedLabel(tokensPerSecond: number | null): string {
  return tokensPerSecond === null ? "–" : `${Math.round(tokensPerSecond)} tok/s`;
}
