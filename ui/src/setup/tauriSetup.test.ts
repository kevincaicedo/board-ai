import assert from "node:assert/strict";
import { test } from "node:test";
import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { TauriBridge, TauriCommand } from "../transport/tauri.ts";
import { firstLaunch } from "./fake.ts";
import { TauriSetupTransport } from "./tauriSetup.ts";
import { parseSetupStatus } from "./transport.ts";

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
  assert.deepEqual(
    seen.map((event) => event.kind),
    ["Progress", "Done"],
  );
  assert.equal(transport.malformedEvents, 2);
});

// D-186: a shell from before `mic_attributed_to` sends none; a blank one names nothing.
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
