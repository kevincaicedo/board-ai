// The settings page's actions through the controller, against the fake app: each sends exactly
// its command, refusals land on their section in plain words, and leaving waits for saves.

import assert from "node:assert/strict";
import { test } from "node:test";
import { SetupController } from "./controller.ts";
import { type FakeSetupTransport, keyStates, presetTransport, withKey } from "./fake.ts";
import { savesInFlight } from "./state.ts";

const KEY = "sk-or-v1-PLANTEDsettingsKEYplanted-0123456789";

function open(): { transport: FakeSetupTransport; controller: SetupController } {
  const transport = presetTransport("settings");
  const controller = new SetupController(transport, transport.current, () => {});
  return { transport, controller };
}

function sent(transport: FakeSetupTransport) {
  return transport.calls.filter((call) => call.kind !== "status");
}

test("switching the talker's provider saves it once; the same pill sends nothing", async () => {
  const { transport, controller } = open();
  await controller.pickTalker("OpenRouter");
  assert.deepEqual(sent(transport), []);
  await controller.pickTalker("Ollama");
  assert.deepEqual(sent(transport), [
    { kind: "save", edit: { kind: "Talker", provider: "Ollama", model: null } },
  ]);
});

test("a model row saves the talker on its provider, and the scribe saves alone", async () => {
  const { transport, controller } = open();
  await controller.pickModel("OpenRouter", "openai/gpt-oss-120b");
  assert.deepEqual(sent(transport), [], "the chosen row again sends nothing");
  await controller.pickModel("OpenRouter", "qwen/qwen3.8-27b");
  await controller.pickScribe("openai/gpt-oss-120b");
  await controller.pickScribe("openai/gpt-oss-120b");
  assert.deepEqual(sent(transport), [
    { kind: "save", edit: { kind: "Talker", provider: "OpenRouter", model: "qwen/qwen3.8-27b" } },
    { kind: "save", edit: { kind: "Scribe", model: "openai/gpt-oss-120b" } },
    { kind: "save", edit: { kind: "Scribe", model: "openai/gpt-oss-120b" } },
  ]);
  transport.refuse.save = "With Claude, memory digests use Claude Haiku.";
  await controller.pickScribe("x/y");
  assert.equal(controller.value.problems.thinking, "With Claude, memory digests use Claude Haiku.");
});

test("a key replaced on the keys section goes out once and leaves the voice alone", async () => {
  const { transport, controller } = open();
  const saving = controller.replaceKey("OpenRouter", KEY);
  assert.equal(controller.value.busy.has("keys"), true);
  assert.equal(savesInFlight(controller.value), true, "leaving waits for it");
  await saving;
  assert.deepEqual(sent(transport), [{ kind: "saveKey", name: "OpenRouter", length: KEY.length }]);
  assert.equal(JSON.stringify(controller.value).includes(KEY), false, "no state holds the key");
});

test("a refused key, a removal and a check land on the keys section", async () => {
  const { transport, controller } = open();
  transport.refuse.saveKey = "The Keychain didn't save the key. Unlock it and try again.";
  await controller.replaceKey("Anthropic", KEY);
  assert.equal(
    controller.value.problems.keys,
    "The Keychain didn't save the key. Unlock it and try again.",
  );
  await controller.removeKey("ElevenLabs");
  assert.equal(controller.value.problems.keys, null, "the next answer clears it");
  assert.deepEqual(
    controller.value.status?.keys.find((key) => key.name === "ElevenLabs"),
    { name: "ElevenLabs", state: "Missing" },
  );
  transport.current = { ...transport.current, keys: keyStates([], "NotChecked") };
  transport.onCheckKeys = () => withKey(transport.current, "Anthropic", "Saved");
  await controller.checkKeys();
  assert.deepEqual(
    sent(transport).map((call) => call.kind),
    ["saveKey", "removeKey", "checkKeys"],
  );
  assert.equal(
    controller.value.status?.keys.find((key) => key.name === "Anthropic")?.state,
    "Saved",
  );
});

test("folders are added through the app's chooser and removed by position", async () => {
  const { transport, controller } = open();
  await controller.addCodeFolder();
  assert.deepEqual(controller.value.status?.code_folders, [
    "~/Documents/Projects",
    "~/code/boardai",
  ]);
  await controller.removeCodeFolder(1);
  assert.deepEqual(sent(transport), [
    { kind: "addCodeFolder" },
    { kind: "save", edit: { kind: "RemoveCodeFolder", index: 1 } },
  ]);
  transport.refuse.addCodeFolder = "The folder chooser didn't open. Try again.";
  await controller.addCodeFolder();
  assert.equal(controller.value.problems.coding, "The folder chooser didn't open. Try again.");
});

test("the harnesses' versions arrive quietly, and their failure says nothing", async () => {
  const { transport, controller } = open();
  await controller.harnessVersions();
  assert.deepEqual(controller.value.status?.harnesses.claude_code, {
    state: "Found",
    version: "2.1.301",
  });
  transport.refuse.harnessVersions = "the app went away";
  await controller.harnessVersions();
  assert.equal(controller.value.problems.coding, null);
  assert.equal(controller.value.busy.size, 0);
});

test("rebuilding the index says what it did; a failure says why; leaving never waits on it", async () => {
  const { transport, controller } = open();
  const rebuilding = controller.rebuildIndex();
  assert.equal(controller.value.busy.has("memory"), true);
  assert.equal(savesInFlight(controller.value), false);
  await rebuilding;
  assert.deepEqual(controller.value.rebuilt, transport.rebuilt);
  transport.refuse.rebuildIndex = "Still rebuilding. It finishes on its own; check again later.";
  await controller.rebuildIndex();
  assert.equal(
    controller.value.problems.memory,
    "Still rebuilding. It finishes on its own; check again later.",
  );
  transport.refuse.openVault = "The vault isn't there yet. BoardAI makes it when a session starts.";
  await controller.openVault();
  assert.match(controller.value.problems.memory ?? "", /vault isn't there yet/);
});

test("leaving waits for a save, then finishes; a blocker keeps the page open", async () => {
  const { transport, controller } = open();
  const saving = controller.pickMode("PushToTalk");
  await controller.leave();
  assert.equal(
    transport.calls.some((call) => call.kind === "finish"),
    false,
    "not while a save is on its way",
  );
  await saving;
  transport.refuse.finish = "Not ready to start yet.";
  await controller.leave();
  assert.equal(controller.value.problems.start, "Not ready to start yet.");
  assert.equal(controller.value.finished, false);
  await controller.leave();
  assert.equal(controller.value.finished, true);
});
