import assert from "node:assert/strict";
import { test } from "node:test";
import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { TauriBridge, TauriCommand } from "../transport/tauri.ts";
import { firstLaunch, PRESETS, presetTransport } from "./fake.ts";
import { recordedListing } from "./fakeModels.ts";
import { parseIndexRebuilt, parseModelListing, parseSetupStatus } from "./parse.ts";
import { TauriSetupTransport } from "./tauriSetup.ts";

interface Invoked {
  command: TauriCommand;
  args: Record<string, unknown>;
}

function bridge(answer: (command: TauriCommand) => unknown) {
  const invoked: Invoked[] = [];
  const channels: ((message: unknown) => void)[] = [];
  const fake: TauriBridge = {
    invoke: async (command, args) => {
      invoked.push({ command, args });
      return answer(command);
    },
    channel: (onMessage) => {
      channels.push(onMessage);
      return { id: channels.length };
    },
  };
  return { fake, invoked, channels };
}

test("each method calls its command with the shell's argument names", async () => {
  const { fake, invoked } = bridge(() => firstLaunch());
  const transport = new TauriSetupTransport(fake);
  await transport.status();
  await transport.save({ kind: "Mic", mode: "PushToTalk" });
  await transport.saveKey("Anthropic", "sk-ant-x");
  await transport.requestMicrophone();
  await transport.finish();
  assert.deepEqual(invoked, [
    { command: "setup_status", args: {} },
    { command: "setup_save", args: { edit: { kind: "Mic", mode: "PushToTalk" } } },
    { command: "setup_key", args: { name: "Anthropic", value: "sk-ant-x" } },
    { command: "setup_microphone", args: {} },
    { command: "setup_finish", args: {} },
  ]);
});

test("a refusal keeps the app's words; a status from another build is refused", async () => {
  const refusing = new TauriSetupTransport(
    bridge(() => {
      throw "That has spaces or line breaks in it. Paste just the key.";
    }).fake,
  );
  await assert.rejects(refusing.saveKey("Anthropic", "a b"), /spaces or line breaks/);
  const mismatched = new TauriSetupTransport(bridge(() => ({ complete: "yes" })).fake);
  await assert.rejects(mismatched.status(), /don't match/);
  assert.equal(parseSetupStatus(firstLaunch()) !== null, true);
  assert.equal(parseSetupStatus({ ...firstLaunch(), blockers: null }), null);
  assert.equal(parseSetupStatus({ ...firstLaunch(), talker: { provider: "Claude" } }), null);
  assert.equal(parseSetupStatus([]), null);
});

test("download events are checked and malformed ones dropped", async () => {
  const { fake, invoked, channels } = bridge(() => null);
  const transport = new TauriSetupTransport(fake);
  const seen: DownloadEvent[] = [];
  await transport.download("silero-vad", (event) => seen.push(event));
  assert.equal(invoked[0]?.command, "setup_download");
  assert.equal(invoked[0]?.args.model, "silero-vad");
  const deliver = channels[0];
  deliver?.({ kind: "Progress", received_bytes: 1, total_bytes: 2 });
  deliver?.({ kind: "Teleport" });
  deliver?.("Done");
  deliver?.({ kind: "Done" });
  // A kind with the wrong fields is as malformed as an unknown kind.
  deliver?.({ kind: "Progress", received_bytes: "half" });
  deliver?.({ kind: "Failed", problem: { kind: "Lava" } });
  deliver?.({ kind: "Failed", problem: { kind: "Server", status: 503 } });
  assert.deepEqual(
    seen.map((event) => event.kind),
    ["Progress", "Done", "Failed"],
  );
  assert.equal(transport.malformedEvents, 4);
});

// D-149: a shell from before `mic_attributed_to` sends none; a blank one names nothing.
test("the app macOS asks about is read at the boundary", async () => {
  const { mic_attributed_to: _left, ...older } = firstLaunch();
  assert.equal(parseSetupStatus(older)?.mic_attributed_to, null);
  assert.equal(
    parseSetupStatus({ ...firstLaunch(), mic_attributed_to: "  " })?.mic_attributed_to,
    null,
  );
  assert.equal(
    parseSetupStatus({ ...firstLaunch(), mic_attributed_to: "Ghostty" })?.mic_attributed_to,
    "Ghostty",
  );
  const { fake, invoked } = bridge(() => null);
  await new TauriSetupTransport(fake).openMicrophoneSettings();
  assert.equal(invoked[0]?.command, "setup_open_microphone_settings");
  const refusing = bridge(() => {
    throw "System Settings didn't open. Open it from the Apple menu instead.";
  });
  await assert.rejects(
    new TauriSetupTransport(refusing.fake).openMicrophoneSettings(),
    /didn't open/,
  );
});

test("the settings page's commands go out with the shell's argument names", async () => {
  const answers: Partial<Record<TauriCommand, unknown>> = {
    setup_models: recordedListing({ kind: "OpenRouter" }),
    setup_rebuild_index: { files_seen: 3, reindexed: 3, skipped: 0, truncated: false },
  };
  const { fake, invoked } = bridge((command) => answers[command] ?? firstLaunch());
  const transport = new TauriSetupTransport(fake);
  const listing = await transport.models({ kind: "OpenRouter" });
  assert.equal(listing.kind, "OpenRouter");
  await transport.removeKey("ElevenLabs");
  await transport.checkKeys();
  await transport.harnessVersions();
  await transport.addCodeFolder();
  await transport.openVault();
  assert.equal((await transport.rebuildIndex()).files_seen, 3);
  assert.deepEqual(invoked, [
    { command: "setup_models", args: { query: { kind: "OpenRouter" } } },
    { command: "setup_remove_key", args: { name: "ElevenLabs" } },
    { command: "setup_check_keys", args: {} },
    { command: "setup_harness_versions", args: {} },
    { command: "setup_add_code_folder", args: {} },
    { command: "setup_open_vault", args: {} },
    { command: "setup_rebuild_index", args: {} },
  ]);
  const refusing = new TauriSetupTransport(
    bridge(() => {
      throw "OpenRouter's list didn't arrive. Check the connection and try again.";
    }).fake,
  );
  await assert.rejects(refusing.models({ kind: "OpenRouter" }), /didn't arrive/);
  const mismatched = new TauriSetupTransport(bridge(() => ({ kind: "Gemini" })).fake);
  await assert.rejects(mismatched.models({ kind: "OpenRouter" }), /don't match/);
});

test("a model list is read row by row; a speed is a number or unknown, never made up", () => {
  const listing = parseModelListing({
    kind: "OpenRouter",
    models: [
      { id: "a/kept", name: "Kept", preset: false, reasoning: { kind: "Never" }, route: [] },
      { id: 7, name: "bad id" },
      { id: "b/no-route", name: "B", preset: true, reasoning: { kind: "Optional" } },
    ],
    left_out: 2,
  });
  assert.deepEqual(
    listing?.kind === "OpenRouter" ? listing.models.map((model) => [model.id, model.route]) : null,
    [
      ["a/kept", []],
      ["b/no-route", []],
    ],
  );
  const providers = parseModelListing({
    kind: "Providers",
    model: "q/x",
    providers: [
      { name: "Cerebras", slug: "cerebras", tools: true, latency_ms_p50: 290 },
      { name: "Odd", slug: "odd", latency_ms_p50: -1, throughput_tokens_per_second_p50: "fast" },
      { slug: "nameless" },
    ],
    left_out: 0,
  });
  assert.deepEqual(
    providers?.kind === "Providers"
      ? providers.providers.map((provider) => [
          provider.name,
          provider.tools,
          provider.latency_ms_p50,
          provider.throughput_tokens_per_second_p50,
        ])
      : null,
    [
      ["Cerebras", true, 290, null],
      ["Odd", false, null, null],
    ],
  );
  assert.equal(parseModelListing({ kind: "Ollama" }), null);
  assert.equal(parseModelListing(null), null);
  assert.equal(
    parseIndexRebuilt({ files_seen: -1, reindexed: 0, skipped: 0, truncated: false }),
    null,
  );
  assert.equal(parseIndexRebuilt({ files_seen: 1, reindexed: 1, skipped: 0 }), null);
});

test("a status without the settings page's fields is from another build", () => {
  const { keys: _keys, ...withoutKeys } = firstLaunch();
  assert.equal(parseSetupStatus(withoutKeys), null);
  assert.equal(parseSetupStatus({ ...firstLaunch(), keys: ["Anthropic"] }), null, "names alone");
  assert.equal(parseSetupStatus({ ...firstLaunch(), code_folders: "~/code" }), null);
  const { elevenlabs: _voice, ...withoutVoice } = firstLaunch();
  assert.equal(parseSetupStatus(withoutVoice), null);
});

// Review 2: the status and model rows were cast (`as unknown as`) after a few spot checks, so a
// shell from another build could hand the screen a state no branch handles. Every field is now
// parsed at the boundary, as protocol/parse.ts does for the session.
test("every status the fake app answers survives parsing unchanged", () => {
  assert.deepEqual(parseSetupStatus(firstLaunch()), firstLaunch());
  for (const preset of PRESETS) {
    const status = presetTransport(preset).current;
    assert.deepEqual(parseSetupStatus(status), status, preset);
  }
});

test("a status with a malformed nested field is from another build", () => {
  const base = firstLaunch();
  const malformed: Record<string, unknown>[] = [
    { talker: { ...base.talker, ollama: { state: "Sleeping" } } },
    { talker: { ...base.talker, ollama: { state: "Reachable", models: [7] } } },
    { talker: { ...base.talker, key: "Perhaps" } },
    { talker: { ...base.talker, provider: "Gemini" } },
    { talker: { ...base.talker, thinking: "Lots" } },
    { talker: { ...base.talker, route: ["cerebras", 3] } },
    { keys: [{ name: "Gemini", state: "Saved" }] },
    { hearing_files: [{ id: "silero-vad", state: "Halfway", have_bytes: 0, size_bytes: 1 }] },
    { voice_files: [{ id: "kokoro", state: "Ready", have_bytes: -1, size_bytes: 1 }] },
    { harnesses: { ...base.harnesses, pi: { state: "Maybe" } } },
    { harnesses: { ...base.harnesses, default: "Cursor" } },
    { blockers: [{ kind: "Unknown" }] },
    { blockers: [{ kind: "TalkerKey" }] },
    { voice: "Siri" },
    { microphone: "Sometimes" },
    { input: { kind: "Named" } },
    { elevenlabs: { model: "Flash" } },
    { input_devices: ["Shure", null] },
    { default_input: 3 },
  ];
  for (const change of malformed) {
    assert.equal(parseSetupStatus({ ...base, ...change }), null, JSON.stringify(change));
  }
});

test("a model or provider row with a malformed field is left out; counts are whole", () => {
  const good = { id: "a/kept", name: "Kept", preset: false, reasoning: { kind: "Never" } };
  const listing = parseModelListing({
    kind: "OpenRouter",
    models: [
      { ...good, context_tokens: null, price: null, route: [] },
      { ...good, id: "b/odd", reasoning: { kind: "Sometimes" } },
      { ...good, id: "c/odd", reasoning: { kind: "Always" } },
      { ...good, id: "d/odd", price: { input_micro_usd_per_million_tokens: "free" } },
      { ...good, id: "e/odd", context_tokens: 1.5 },
    ],
    left_out: 0,
  });
  assert.deepEqual(
    listing?.kind === "OpenRouter" ? listing.models.map((model) => model.id) : null,
    ["a/kept"],
  );
  const providers = parseModelListing({
    kind: "Providers",
    model: "q/x",
    providers: [
      { name: "Cerebras", slug: "cerebras", quantization: 16 },
      { name: "Odd", slug: "odd", max_output_tokens: "many" },
    ],
    left_out: 0,
  });
  assert.equal(providers?.kind === "Providers" ? providers.providers.length : -1, 0);
  assert.equal(
    parseIndexRebuilt({ files_seen: 1.5, reindexed: 0, skipped: 0, truncated: false }),
    null,
  );
  assert.equal(parseModelListing({ kind: "OpenRouter", models: [], left_out: -1 }), null);
});
