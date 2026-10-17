import assert from "node:assert/strict";
import { test } from "node:test";
import type { DownloadProblem } from "../protocol/generated/DownloadProblem.ts";
import type { SetupBlocker } from "../protocol/generated/SetupBlocker.ts";
import { firstLaunch } from "./fake.ts";
import type { GroupProgress } from "./state.ts";
import {
  downloadProblem,
  FOOTER_FIRST_RUN,
  groupSize,
  inputLabel,
  keyHint,
  keyLabel,
  keyNote,
  MIC_DENIED,
  micDenied,
  micStep,
  percent,
  startHint,
  startLabel,
  thinkingStep,
  voiceStep,
} from "./words.ts";

const MIB = 1024 * 1024;

function progress(receivedBytes: number, totalBytes: number, ready = false): GroupProgress {
  return { receivedBytes, totalBytes, ready, running: !ready, problem: null };
}

test("model sizes read as the artboard writes them", () => {
  assert.equal(groupSize(progress(0, 312 * MIB)), "312 MB");
  assert.equal(groupSize(progress(201 * MIB, 312 * MIB)), "201 of 312 MB");
  assert.equal(groupSize(progress(142 * MIB, 142 * MIB, true)), "ready · 142 MB");
  assert.equal(percent(progress(201 * MIB, 312 * MIB)), 64);
  assert.equal(percent(progress(5, 0)), 0);
  assert.equal(percent(progress(10, 5)), 100);
  assert.equal(percent(progress(0, 5, true)), 100);
});

test("every blocker has a line under Start, and a key is named by its provider", () => {
  const blockers: SetupBlocker[] = [
    { kind: "SettingsProblem" },
    { kind: "TalkerKey", provider: "Claude" },
    { kind: "TalkerKey", provider: "Other" },
    { kind: "OllamaUnreachable" },
    { kind: "OllamaModelMissing", model: "gemma4:latest" },
    { kind: "UnknownHearingModel", model: "large" },
    { kind: "HearingNotReady" },
    { kind: "MicrophoneNotAllowed" },
    { kind: "MicrophoneDenied" },
  ];
  for (const blocker of blockers) {
    const line = startHint({ ...firstLaunch(), blockers: [blocker] });
    assert.ok(line !== null && line.length > 10, blocker.kind);
  }
  assert.equal(
    startHint({ ...firstLaunch(), blockers: [{ kind: "TalkerKey", provider: "OpenRouter" }] }),
    "Waiting for: your OpenRouter API key.",
  );
  assert.match(
    startHint({
      ...firstLaunch(),
      settings_problem: "settings.toml `mic`: must be `open` or `push_to_talk`",
      blockers: [{ kind: "SettingsProblem" }],
    }) ?? "",
    /`mic`/,
  );
  assert.equal(startHint({ ...firstLaunch(), blockers: [] }), null);
});

test("every download problem is said in plain words", () => {
  const problems: DownloadProblem[] = [
    { kind: "Network" },
    { kind: "Server", status: 503 },
    { kind: "Unexpected" },
    { kind: "Corrupt" },
    { kind: "Disk" },
  ];
  const lines = problems.map(downloadProblem);
  assert.equal(new Set(lines).size, problems.length);
  assert.match(lines[1] ?? "", /503/);
});

test("a saved key's field says so and never shows the key", () => {
  assert.equal(keyHint("Anthropic", false), "sk-ant-…");
  assert.equal(keyHint("Anthropic", true), "Saved · paste to replace");
});

// 11b · First run states: what each card and the Start row say when something is missing.
test("Start waits with every blocker named once, in one line (11b)", () => {
  const waiting = (...blockers: SetupBlocker[]): string | null =>
    startHint({ ...firstLaunch(), blockers });
  assert.equal(
    waiting({ kind: "HearingNotReady" }, { kind: "MicrophoneNotAllowed" }),
    "Waiting for: the hearing model, and the microphone.",
  );
  assert.equal(
    waiting(
      { kind: "TalkerKey", provider: "Claude" },
      { kind: "HearingNotReady" },
      { kind: "MicrophoneNotAllowed" },
      { kind: "MicrophoneDenied" },
    ),
    "Waiting for: your Anthropic API key, the hearing model, and the microphone.",
  );
  assert.equal(waiting({ kind: "OllamaUnreachable" }), "Waiting for: Ollama to start.");
});

test("each card names its state on its step line (11b)", () => {
  const status = firstLaunch();
  assert.equal(thinkingStep(status), "1 · THINKING");
  assert.equal(
    thinkingStep({ ...status, talker: { ...status.talker, key: "Saved" } }),
    "1 · THINKING · KEY SAVED",
  );
  assert.equal(voiceStep(false), "2 · VOICE AND HEARING");
  assert.equal(voiceStep(true), "2 · VOICE AND HEARING · ERROR");
  assert.equal(micStep("Undetermined"), "3 · MICROPHONE");
  assert.equal(micStep("Denied"), "3 · MICROPHONE · DENIED");
});

test("the cards' words match the states frame (11b)", () => {
  assert.equal(
    downloadProblem({ kind: "Network" }),
    "The download stopped: no connection. It picks up where it left off.",
  );
  assert.equal(
    MIC_DENIED,
    "Turn the microphone on in System Settings › Privacy & Security › Microphone, then come back. This screen notices on its own.",
  );
  assert.equal(keyNote(true), "The key never comes back to this screen: only that one is saved.");
  assert.equal(keyNote(false), "Saved to Keychain. Never written to a file or a log.");
  assert.equal(keyLabel("ElevenLabs"), "ElevenLabs API key (optional voice)");
  assert.equal(keyHint("ElevenLabs", false), "Paste, then press Return");
  assert.equal(inputLabel({ kind: "SystemDefault" }), "System default");
  assert.equal(inputLabel({ kind: "BuiltIn" }), "Built-in microphone");
  assert.equal(FOOTER_FIRST_RUN, "Reopen any time with ⌘, or by saying “open settings”.");
});

test("over a running session the button goes back to talking", () => {
  assert.equal(startLabel(false), "Start talking");
  assert.equal(startLabel(true), "Back to talking");
});

// D-186: started from another app, the denied card names the switch macOS actually asks about.
test("a denied microphone names the app macOS asks about", () => {
  assert.equal(micDenied(null), MIC_DENIED);
  assert.equal(
    micDenied("Visual Studio Code"),
    "macOS asks about Visual Studio Code here, because BoardAI was started from it. Turn on Visual Studio Code in System Settings › Privacy & Security › Microphone, then come back.",
  );
});
