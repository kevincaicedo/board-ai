import assert from "node:assert/strict";
import { test } from "node:test";
import type { DownloadProblem } from "../protocol/generated/DownloadProblem.ts";
import type { SetupBlocker } from "../protocol/generated/SetupBlocker.ts";
import { firstLaunch } from "./fake.ts";
import type { GroupProgress } from "./state.ts";
import { downloadProblem, groupSize, keyHint, percent, startHint } from "./words.ts";

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
    "Paste your OpenRouter API key to start.",
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
