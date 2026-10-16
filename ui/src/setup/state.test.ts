import assert from "node:assert/strict";
import { test } from "node:test";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { firstLaunch } from "./fake.ts";
import {
  canStart,
  filesToFetch,
  groupProgress,
  initialState,
  needsPolling,
  reduce,
  type SetupAction,
  type SetupState,
} from "./state.ts";

const MIB = 1024 * 1024;

function readyStatus(): SetupStatus {
  const status = firstLaunch();
  return {
    ...status,
    talker: { ...status.talker, key: "Saved" },
    hearing_files: status.hearing_files.map((file) => ({
      ...file,
      state: "Ready",
      have_bytes: file.size_bytes,
    })),
    microphone: "Granted",
    blockers: [],
  };
}

function run(state: SetupState, ...actions: SetupAction[]): SetupState {
  return actions.reduce(reduce, state);
}

test("start waits for a status, for the app's blockers and for every request in flight", () => {
  assert.equal(canStart(initialState(null)), false);
  assert.equal(canStart(initialState(firstLaunch())), false, "the app names blockers");
  const ready = initialState(readyStatus());
  assert.equal(canStart(ready), true);
  const saving = reduce(ready, { type: "busy", card: "thinking" });
  assert.equal(canStart(saving), false, "a key is being saved");
  const answered = reduce(saving, { type: "answered", card: "thinking", status: readyStatus() });
  assert.equal(canStart(answered), true);
  const starting = reduce(answered, { type: "busy", card: "start" });
  assert.equal(canStart(starting), false, "no second start while the first is in flight");
  const started = reduce(starting, { type: "finished" });
  assert.equal(started.finished, true);
  assert.equal(started.busy.size, 0);
  assert.equal(canStart(started), false);
});

test("a refusal lands on its own card and the card's next answer clears it", () => {
  const state = run(
    initialState(firstLaunch()),
    { type: "busy", card: "microphone" },
    { type: "refused", card: "microphone", reason: "settings.toml `mic`: must be …" },
  );
  assert.equal(state.problems.microphone, "settings.toml `mic`: must be …");
  assert.equal(state.problems.thinking, null);
  assert.equal(state.busy.size, 0);
  const again = run(
    state,
    { type: "busy", card: "microphone" },
    { type: "answered", card: "microphone", status: readyStatus() },
  );
  assert.equal(again.problems.microphone, null);
});

test("an answer older than the status on screen keeps the newer status", () => {
  const newer = readyStatus();
  const state = run(
    initialState(firstLaunch()),
    { type: "busy", card: "voice" },
    { type: "loaded", status: newer },
    { type: "answered", card: "voice", status: null },
  );
  assert.equal(state.status, newer);
  assert.equal(state.busy.size, 0);
});

test("a download moves from the file's partial size through progress to done or failed", () => {
  const partial: SetupStatus = {
    ...firstLaunch(),
    hearing_files: [
      { id: "whisper-base.en", state: "Partial", have_bytes: 40 * MIB, size_bytes: 140 * MIB },
      { id: "silero-vad", state: "Missing", have_bytes: 0, size_bytes: 2 * MIB },
    ],
  };
  let state = run(initialState(partial), { type: "transferStarted", model: "whisper-base.en" });
  assert.deepEqual(state.transfers.get("whisper-base.en"), {
    receivedBytes: 40 * MIB,
    totalBytes: 140 * MIB,
    live: true,
    problem: null,
  });
  state = reduce(state, {
    type: "transfer",
    model: "whisper-base.en",
    event: { kind: "Progress", received_bytes: 100 * MIB, total_bytes: 140 * MIB },
  });
  const hearing = groupProgress(partial.hearing_files, state.transfers);
  assert.equal(hearing.receivedBytes, 100 * MIB);
  assert.equal(hearing.totalBytes, 142 * MIB);
  assert.equal(hearing.running, true);
  assert.equal(hearing.ready, false);

  const done = reduce(state, {
    type: "transfer",
    model: "whisper-base.en",
    event: { kind: "Done" },
  });
  assert.deepEqual(done.transfers.get("whisper-base.en"), {
    receivedBytes: 140 * MIB,
    totalBytes: 140 * MIB,
    live: false,
    problem: null,
  });

  const failed = reduce(state, {
    type: "transfer",
    model: "whisper-base.en",
    event: { kind: "Failed", problem: { kind: "Corrupt" } },
  });
  const stopped = groupProgress(partial.hearing_files, failed.transfers);
  assert.equal(stopped.running, false);
  assert.match(stopped.problem ?? "", /checksum/);
  assert.equal(stopped.receivedBytes, 100 * MIB, "the bar stays where it stopped");
});

test("files to fetch: hearing always, Kokoro only as the voice, never twice or after a failure", () => {
  const status = firstLaunch();
  assert.deepEqual(filesToFetch(initialState(status)), [
    "whisper-base.en",
    "silero-vad",
    "kokoro",
    "kokoro-voice-af-heart",
    "misaki-us-gold",
  ]);
  const elevenLabs: SetupStatus = { ...status, voice: "ElevenLabs" };
  assert.deepEqual(filesToFetch(initialState(elevenLabs)), ["whisper-base.en", "silero-vad"]);

  const busy = run(
    initialState(elevenLabs),
    { type: "transferStarted", model: "whisper-base.en" },
    {
      type: "transfer",
      model: "silero-vad",
      event: { kind: "Failed", problem: { kind: "Network" } },
    },
  );
  assert.deepEqual(filesToFetch(busy), [], "one is running, the other failed");
  const retried = reduce(busy, { type: "retry" });
  assert.deepEqual(filesToFetch(retried), ["silero-vad"]);
  assert.equal(filesToFetch(reduce(retried, { type: "finished" })).length, 0);

  const refused = reduce(initialState(elevenLabs), {
    type: "transferRefused",
    model: "silero-vad",
    reason: "BoardAI can't find its settings folder.",
  });
  assert.equal(
    groupProgress(elevenLabs.hearing_files, refused.transfers).problem,
    "BoardAI can't find its settings folder.",
  );
  assert.deepEqual(filesToFetch(refused), ["whisper-base.en"]);
});

test("a file downloading for an earlier screen is polled, not fetched again", () => {
  const status: SetupStatus = {
    ...firstLaunch(),
    voice: "System",
    hearing_files: [
      { id: "whisper-base.en", state: "Downloading", have_bytes: 9 * MIB, size_bytes: 140 * MIB },
      { id: "silero-vad", state: "Ready", have_bytes: 2 * MIB, size_bytes: 2 * MIB },
    ],
  };
  const state = initialState(status);
  assert.deepEqual(filesToFetch(state), []);
  assert.equal(needsPolling(state), true);
  assert.equal(groupProgress(status.hearing_files, state.transfers).running, true);
  const lost = run(
    state,
    { type: "transferStarted", model: "whisper-base.en" },
    { type: "transferLost", model: "whisper-base.en" },
  );
  assert.equal(lost.transfers.get("whisper-base.en")?.live, false);
  assert.equal(needsPolling(lost), true);
  assert.equal(needsPolling(initialState(readyStatus())), false);
});

test("a group is ready only when every file is verified", () => {
  const status = readyStatus();
  const hearing = groupProgress(status.hearing_files, new Map());
  assert.equal(hearing.ready, true);
  assert.equal(hearing.running, false);
  assert.equal(hearing.receivedBytes, hearing.totalBytes);
  const voice = groupProgress(status.voice_files, new Map());
  assert.equal(voice.ready, false);
  assert.equal(voice.receivedBytes, 0);
  assert.equal(groupProgress([], new Map()).ready, false);
});

test("the ElevenLabs key field closes once the voice card is answered", () => {
  const state = run(
    initialState(firstLaunch()),
    { type: "voiceKey", open: true },
    { type: "busy", card: "voice" },
  );
  assert.equal(state.addingVoiceKey, true);
  const closed = reduce(state, { type: "answered", card: "voice", status: readyStatus() });
  assert.equal(closed.addingVoiceKey, false);
});
