import assert from "node:assert/strict";
import { test } from "node:test";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { type Schedule, SETUP_POLL_MS, SetupController } from "./controller.ts";
import { FakeSetupTransport, firstLaunch } from "./fake.ts";

const KEY = "sk-ant-api03-PLANTEDuiKEYplantedUIkey-0123456789";

/** Lets every pending promise callback run. */
async function settle(): Promise<void> {
  for (let round = 0; round < 5; round += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

function ready(status: SetupStatus): SetupStatus {
  return {
    ...status,
    talker: { ...status.talker, key: "Saved" },
    keys: ["Anthropic"],
    hearing_files: status.hearing_files.map((file) => ({
      ...file,
      state: "Ready",
      have_bytes: file.size_bytes,
    })),
    voice_files: status.voice_files.map((file) => ({
      ...file,
      state: "Ready",
      have_bytes: file.size_bytes,
    })),
    microphone: "Granted",
    blockers: [],
  };
}

function setup(status: SetupStatus = firstLaunch()) {
  const transport = new FakeSetupTransport(status);
  const scheduled: { delayMs: number; run: () => void }[] = [];
  const schedule: Schedule = (delayMs, run) => {
    scheduled.push({ delayMs, run });
  };
  const controller = new SetupController(transport, null, schedule);
  return { transport, controller, scheduled };
}

test("loading reads the status and starts every download the session needs", async () => {
  const { transport, controller } = setup();
  await controller.load();
  await settle();
  assert.equal(controller.value.status?.complete, false);
  assert.deepEqual(
    transport.calls.filter((call) => call.kind === "download").map((call) => call.model),
    ["whisper-base.en", "silero-vad", "kokoro", "kokoro-voice-af-heart", "misaki-us-gold"],
  );
  // Another status while they run starts nothing twice.
  await controller.refresh();
  assert.equal(transport.calls.filter((call) => call.kind === "download").length, 5);
});

test("a status that can't be read says so in plain words", async () => {
  const { transport, controller } = setup();
  transport.refuse.status = "BoardAI can't find its settings folder.";
  await controller.load();
  assert.equal(controller.value.status, null);
  assert.equal(controller.value.loadProblem, "BoardAI can't find its settings folder.");
});

test("a key goes out once, and nothing in the screen's state holds it", async () => {
  const { transport, controller } = setup();
  await controller.load();
  transport.onKey = () => ({
    ...firstLaunch(),
    talker: { ...firstLaunch().talker, key: "Saved" },
    keys: ["Anthropic"],
  });
  const saving = controller.saveKey("Anthropic", KEY);
  assert.equal(controller.value.busy.has("thinking"), true);
  await saving;
  assert.deepEqual(
    transport.calls.filter((call) => call.kind === "saveKey"),
    [{ kind: "saveKey", name: "Anthropic", length: KEY.length }],
  );
  assert.equal(controller.value.status?.talker.key, "Saved");
  assert.equal(controller.value.busy.size, 0);
  const shown = JSON.stringify(controller.value, (_key, value: unknown) =>
    value instanceof Map ? [...value] : value instanceof Set ? [...value] : value,
  );
  assert.equal(shown.includes("PLANTED"), false);
});

test("an ElevenLabs key also makes ElevenLabs the voice", async () => {
  const { transport, controller } = setup();
  await controller.load();
  controller.openVoiceKey(true);
  await controller.saveKey("ElevenLabs", "sk_planted");
  assert.deepEqual(
    transport.calls.filter((call) => call.kind === "saveKey" || call.kind === "save"),
    [
      { kind: "saveKey", name: "ElevenLabs", length: 10 },
      { kind: "save", edit: { kind: "Voice", engine: "ElevenLabs" } },
    ],
  );
  assert.equal(controller.value.addingVoiceKey, false);
});

test("a refused save shows on its card only; picking the current choice sends nothing", async () => {
  const { transport, controller } = setup();
  await controller.load();
  await controller.pickTalker("Claude");
  await controller.pickMode("Open");
  assert.equal(transport.calls.filter((call) => call.kind === "save").length, 0);
  transport.refuse.save = "settings.toml `talker.kind`: must be `anthropic` or `openai_compatible`";
  await controller.pickTalker("Ollama");
  assert.match(controller.value.problems.thinking ?? "", /talker\.kind/);
  assert.equal(controller.value.problems.microphone, null);
  assert.equal(controller.value.busy.size, 0);
  await controller.pickMode("PushToTalk");
  assert.deepEqual(transport.calls.at(-1), {
    kind: "save",
    edit: { kind: "Mic", mode: "PushToTalk" },
  });
});

test("a finished download asks for the status again; a failed one waits for Try again", async () => {
  const { transport, controller } = setup();
  await controller.load();
  await settle();
  const statusCalls = () => transport.calls.filter((call) => call.kind === "status").length;
  const before = statusCalls();
  transport.current = {
    ...firstLaunch(),
    hearing_files: firstLaunch().hearing_files.map((file) =>
      file.id === "whisper-base.en"
        ? { ...file, state: "Ready", have_bytes: file.size_bytes }
        : file,
    ),
  };
  const whisper = transport.downloads.get("whisper-base.en");
  whisper?.emit({ kind: "Done" });
  whisper?.resolve();
  await settle();
  assert.equal(statusCalls(), before + 1);

  const silero = transport.downloads.get("silero-vad");
  silero?.emit({ kind: "Failed", problem: { kind: "Network" } });
  silero?.reject("The download stopped.");
  await settle();
  const transfer = controller.value.transfers.get("silero-vad");
  assert.match(transfer?.problem ?? "", /connection/);
  const downloads = () => transport.calls.filter((call) => call.kind === "download").length;
  const count = downloads();
  await controller.refresh();
  assert.equal(downloads(), count, "no automatic retry");
  controller.retryDownloads();
  assert.equal(downloads(), count + 1);
});

test("a file already downloading elsewhere is polled until it is done", async () => {
  const status: SetupStatus = { ...firstLaunch(), voice: "System" };
  const { transport, controller, scheduled } = setup(status);
  await controller.load();
  await settle();
  transport.current = {
    ...status,
    hearing_files: status.hearing_files.map((file) => ({ ...file, state: "Downloading" })),
  };
  transport.downloads.get("whisper-base.en")?.resolve();
  await settle();
  assert.equal(scheduled.length, 1);
  assert.equal(scheduled[0]?.delayMs, SETUP_POLL_MS);
  transport.current = ready(status);
  scheduled[0]?.run();
  await settle();
  assert.equal(scheduled.length, 1, "no more polling once the file is ready");
});

test("start finishes only when the app names no blocker, and says why it couldn't", async () => {
  const { transport, controller } = setup();
  await controller.load();
  await controller.start();
  assert.equal(transport.calls.filter((call) => call.kind === "finish").length, 0);

  transport.current = ready(firstLaunch());
  await controller.refresh();
  transport.refuse.finish = "Not ready to start yet.";
  await controller.start();
  assert.equal(controller.value.problems.start, "Not ready to start yet.");
  assert.equal(controller.value.finished, false);

  await controller.start();
  assert.equal(controller.value.finished, true);
  assert.equal(transport.calls.filter((call) => call.kind === "finish").length, 2);
});

test("an answer that arrives after a newer status doesn't replace it", async () => {
  const { transport, controller } = setup();
  await controller.load();
  let answer: (status: SetupStatus) => void = () => {};
  const slow = new Promise<SetupStatus>((resolve) => {
    answer = resolve;
  });
  transport.requestMicrophone = () => slow;
  const asking = controller.allowMicrophone();
  const newer = ready(firstLaunch());
  transport.current = newer;
  await controller.refresh();
  answer({ ...firstLaunch(), microphone: "Denied" });
  await asking;
  assert.equal(controller.value.status?.microphone, "Granted");
  assert.equal(controller.value.busy.size, 0);
});

test("leaving settings asks the app even with a blocker, which decides; start does not", async () => {
  const { transport, controller } = setup({ ...ready(firstLaunch()), complete: true });
  await controller.load();
  transport.current = {
    ...ready(firstLaunch()),
    complete: true,
    blockers: [{ kind: "OllamaUnreachable" }],
  };
  await controller.refresh();
  await controller.start();
  assert.equal(transport.calls.filter((call) => call.kind === "finish").length, 0);
  transport.refuse.finish = "Not ready to start yet.";
  await controller.leave();
  assert.equal(controller.value.problems.start, "Not ready to start yet.");
  assert.equal(controller.value.finished, false);
  await controller.leave();
  assert.equal(controller.value.finished, true, "nothing was saved, so it just closes");
  assert.equal(transport.calls.filter((call) => call.kind === "finish").length, 2);
});

test("opening the microphone pane goes to the app; a failure is said on the card", async () => {
  const { transport, controller } = setup({ ...firstLaunch(), microphone: "Denied" });
  await controller.load();
  await controller.openMicrophoneSettings();
  assert.ok(transport.calls.some((call) => call.kind === "openMicrophoneSettings"));
  assert.equal(controller.value.problems.microphone, null);
  transport.refuse.openMicrophoneSettings = "System Settings didn't open.";
  await controller.openMicrophoneSettings();
  assert.equal(controller.value.problems.microphone, "System Settings didn't open.");
});
