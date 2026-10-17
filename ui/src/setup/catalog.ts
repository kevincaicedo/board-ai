// The settings page's model lists (D-153): the talker's provider's list, and the provider lists
// of the models the page shows numbers for (the presets and the chosen model). Answers go into
// one signal; an answer for a provider the page has since left is dropped.

import { type Signal, signal } from "@preact/signals";
import type { ModelInfo } from "../protocol/generated/ModelInfo.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { TalkerProvider } from "../protocol/generated/TalkerProvider.ts";
import type { Source } from "./models.ts";
import { reason, type SetupTransport } from "./transport.ts";

/** Provider lists kept while the page is open; past it the oldest is dropped and asked again. */
export const PROVIDER_LISTS_KEPT_MAX = 16;

export type Listing =
  | { readonly kind: "idle" }
  | { readonly kind: "loading" }
  | { readonly kind: "loaded"; readonly models: readonly ModelInfo[]; readonly leftOut: number }
  | { readonly kind: "failed"; readonly reason: string };

export interface CatalogState {
  /** The list on show; `null` for a provider without one (Claude, or settings.toml's own). */
  readonly source: Source | null;
  readonly listing: Listing;
  /** Provider lists by model id, oldest first. */
  readonly providers: ReadonlyMap<string, readonly ModelProvider[]>;
  readonly search: string;
}

export const EMPTY_CATALOG: CatalogState = {
  source: null,
  listing: { kind: "idle" },
  providers: new Map(),
  search: "",
};

/** Which list a talker provider picks from. */
export function sourceFor(provider: TalkerProvider): Source | null {
  switch (provider) {
    case "OpenRouter":
      return "OpenRouter";
    case "Ollama":
      return "Ollama";
    case "Claude":
    case "Other":
      return null;
  }
}

/** `providers` with `model`'s list added last, the oldest dropped past the bound. */
export function keepProviders(
  providers: ReadonlyMap<string, readonly ModelProvider[]>,
  model: string,
  list: readonly ModelProvider[],
): ReadonlyMap<string, readonly ModelProvider[]> {
  const next = new Map(providers);
  next.delete(model);
  next.set(model, list);
  while (next.size > PROVIDER_LISTS_KEPT_MAX) {
    const oldest = next.keys().next();
    if (oldest.done === true) {
      break;
    }
    next.delete(oldest.value);
  }
  return next;
}

export class ModelCatalog {
  readonly state: Signal<CatalogState> = signal(EMPTY_CATALOG);
  readonly #transport: Pick<SetupTransport, "models">;
  /** Lists asked for, so an answer for a source the page has left never lands. */
  #ticket = 0;
  readonly #asking = new Set<string>();

  constructor(transport: Pick<SetupTransport, "models">) {
    this.#transport = transport;
  }

  get value(): CatalogState {
    return this.state.value;
  }

  /** Shows `source`'s list, fetching it unless it is already here or on its way. */
  show(source: Source | null): Promise<void> {
    const current = this.state.value;
    if (current.source === source && current.listing.kind !== "failed") {
      return Promise.resolve();
    }
    return this.#fetch(source);
  }

  /** "Try again" after a list didn't arrive. */
  retry(): Promise<void> {
    return this.#fetch(this.state.value.source);
  }

  search(text: string): void {
    this.state.value = { ...this.state.value, search: text };
  }

  /** Loads who serves an OpenRouter model, once; a failure only leaves its numbers out. */
  async loadProviders(model: string): Promise<void> {
    if (this.state.value.providers.has(model) || this.#asking.has(model)) {
      return;
    }
    this.#asking.add(model);
    try {
      const listing = await this.#transport.models({ kind: "Providers", model });
      if (listing.kind === "Providers") {
        const providers = keepProviders(this.state.value.providers, model, listing.providers);
        this.state.value = { ...this.state.value, providers };
      }
    } catch {
      // The row keeps the list's own price and context and shows no speed.
    } finally {
      this.#asking.delete(model);
    }
  }

  async #fetch(source: Source | null): Promise<void> {
    const ticket = ++this.#ticket;
    if (source === null) {
      this.state.value = { ...this.state.value, source, listing: { kind: "idle" } };
      return;
    }
    this.state.value = { ...this.state.value, source, listing: { kind: "loading" } };
    let listing: Listing;
    try {
      const answer = await this.#transport.models({ kind: source });
      listing =
        answer.kind === "Providers"
          ? { kind: "failed", reason: "BoardAI's window and app don't match. Reinstall BoardAI." }
          : {
              kind: "loaded",
              models: answer.models,
              leftOut: answer.kind === "OpenRouter" ? answer.left_out : 0,
            };
    } catch (error) {
      listing = { kind: "failed", reason: reason(error) };
    }
    if (ticket !== this.#ticket) {
      return;
    }
    this.state.value = { ...this.state.value, listing };
    if (source === "OpenRouter" && listing.kind === "loaded") {
      for (const model of listing.models.filter((entry) => entry.preset)) {
        void this.loadProviders(model.id);
      }
    }
  }
}
