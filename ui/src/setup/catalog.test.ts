import assert from "node:assert/strict";
import { test } from "node:test";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelQuery } from "../protocol/generated/ModelQuery.ts";
import { keepProviders, ModelCatalog, PROVIDER_LISTS_KEPT_MAX, sourceFor } from "./catalog.ts";
import { recordedListing } from "./fakeModels.ts";

/** Lets every pending promise callback run. */
async function settle(): Promise<void> {
  for (let round = 0; round < 5; round += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

/** A list source whose answers the test releases by hand. */
class Lists {
  readonly asked: ModelQuery[] = [];
  readonly #waiting: {
    query: ModelQuery;
    answer: (listing: ModelListing) => void;
    fail: (error: unknown) => void;
  }[] = [];

  models(query: ModelQuery): Promise<ModelListing> {
    this.asked.push(query);
    return new Promise((answer, fail) => this.#waiting.push({ query, answer, fail }));
  }

  answer(kind: ModelQuery["kind"]): void {
    const index = this.#waiting.findIndex((entry) => entry.query.kind === kind);
    const entry = this.#waiting.splice(index, 1)[0];
    entry?.answer(recordedListing(entry.query));
  }

  fail(kind: ModelQuery["kind"], reason: string): void {
    const index = this.#waiting.findIndex((entry) => entry.query.kind === kind);
    this.#waiting.splice(index, 1)[0]?.fail(reason);
  }
}

/** The listing's kind, read fresh (an assertion would otherwise narrow the getter's type). */
function kindOf(catalog: ModelCatalog): string {
  return catalog.value.listing.kind;
}

test("each talker provider picks its list; Claude and settings.toml's own have none", () => {
  assert.equal(sourceFor("OpenRouter"), "OpenRouter");
  assert.equal(sourceFor("Ollama"), "Ollama");
  assert.equal(sourceFor("Claude"), null);
  assert.equal(sourceFor("Other"), null);
});

test("OpenRouter's list loads, then who serves each preset", async () => {
  const lists = new Lists();
  const catalog = new ModelCatalog(lists);
  const showing = catalog.show("OpenRouter");
  assert.equal(kindOf(catalog), "loading");
  lists.answer("OpenRouter");
  await showing;
  assert.equal(kindOf(catalog), "loaded");
  assert.deepEqual(
    lists.asked.slice(1),
    [
      "openai/gpt-oss-120b",
      "qwen/qwen3.8-27b",
      "deepseek/deepseek-v4.1-flash",
      "openai/gpt-6-luna",
      "anthropic/claude-haiku-5.5",
      "google/gemini-3.8-flash",
    ].map((model) => ({ kind: "Providers", model })),
    "only the six presets' providers, for their route's numbers",
  );
  for (let preset = 0; preset < 6; preset += 1) {
    lists.answer("Providers");
  }
  await settle();
  assert.equal(catalog.value.providers.get("qwen/qwen3.8-27b")?.length, 4);
  await catalog.show("OpenRouter");
  assert.equal(lists.asked.length, 7, "a list on show is not asked for again");
  await catalog.loadProviders("qwen/qwen3.8-27b");
  assert.equal(lists.asked.length, 7, "nor is a provider list already here");
});

test("switching provider drops the answer for the one left behind", async () => {
  const lists = new Lists();
  const catalog = new ModelCatalog(lists);
  const openRouter = catalog.show("OpenRouter");
  const ollama = catalog.show("Ollama");
  lists.answer("Ollama");
  await ollama;
  lists.answer("OpenRouter");
  await openRouter;
  const listing = catalog.value.listing;
  assert.equal(catalog.value.source, "Ollama");
  assert.equal(listing.kind === "loaded" ? listing.models[0]?.id : null, "gemma4:latest");
  await catalog.show(null);
  assert.deepEqual(catalog.value.listing, { kind: "idle" });
});

test("a list that doesn't arrive says so, and Try again asks once more", async () => {
  const lists = new Lists();
  const catalog = new ModelCatalog(lists);
  const showing = catalog.show("OpenRouter");
  lists.fail("OpenRouter", "OpenRouter's list didn't arrive. Check the connection and try again.");
  await showing;
  assert.deepEqual(catalog.value.listing, {
    kind: "failed",
    reason: "OpenRouter's list didn't arrive. Check the connection and try again.",
  });
  const again = catalog.retry();
  assert.equal(kindOf(catalog), "loading");
  lists.answer("OpenRouter");
  await again;
  assert.equal(kindOf(catalog), "loaded");
  // A provider list that doesn't arrive leaves only its numbers out, quietly.
  lists.fail("Providers", "down");
  await settle();
  assert.equal(catalog.value.providers.has("openai/gpt-oss-120b"), false);
});

test("provider lists kept are bounded, oldest out", () => {
  let kept = new Map();
  for (let index = 0; index <= PROVIDER_LISTS_KEPT_MAX; index += 1) {
    kept = new Map(keepProviders(kept, `author/model-${index}`, []));
  }
  assert.equal(kept.size, PROVIDER_LISTS_KEPT_MAX);
  assert.equal(kept.has("author/model-0"), false);
  assert.equal(kept.has(`author/model-${PROVIDER_LISTS_KEPT_MAX}`), true);
});

test("the search is kept with the list", () => {
  const catalog = new ModelCatalog(new Lists());
  catalog.search("qwen fast");
  assert.equal(catalog.value.search, "qwen fast");
});
