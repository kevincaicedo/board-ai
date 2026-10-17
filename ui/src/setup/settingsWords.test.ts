import assert from "node:assert/strict";
import { test } from "node:test";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { firstLaunch, keyStates, presetTransport, withKey } from "./fake.ts";
import { RECORDED_MODELS, RECORDED_QWEN_PROVIDERS } from "./fakeModels.ts";
import {
  activeSection,
  drawsDiagrams,
  elevenLabsPickable,
  harnessLine,
  inputRows,
  keyRow,
  permissionHelp,
  permissionLine,
  providerNote,
  providerRow,
  rebuiltLine,
  scribeChangeable,
  scribeLine,
  sectionBadge,
  servedBy,
  talkerKeyMissing,
  thinkingLabel,
  voiceStatus,
} from "./settingsWords.ts";
import { groupProgress } from "./state.ts";

function settings(): SetupStatus {
  return presetTransport("settings").current;
}

test("each key reads as saved, not set, not checked or locked, with its actions", () => {
  assert.deepEqual(keyRow("Saved"), {
    tone: "ok",
    status: "Saved",
    primary: "Replace",
    secondary: "Remove",
  });
  assert.deepEqual(keyRow("Missing"), {
    tone: "quiet",
    status: "Not set",
    primary: "Add a key",
    secondary: null,
  });
  assert.equal(keyRow("NotChecked").status, "Not checked while talking");
  assert.equal(keyRow("NotChecked").secondary, "Check");
  assert.equal(keyRow("Unreadable").tone, "off");
  assert.match(keyRow("Unreadable").status, /Unlock it/);
});

test("the provider pills say what each needs", () => {
  const status = settings();
  assert.equal(providerNote("Claude", status), "key needed");
  assert.equal(providerNote("OpenRouter", status), "key saved");
  assert.equal(providerNote("Ollama", status), "gemma4:latest");
  const unchecked = { ...status, keys: keyStates([], "NotChecked") };
  assert.equal(providerNote("OpenRouter", unchecked), "key not checked");
  const down: SetupStatus = {
    ...status,
    talker: { ...status.talker, ollama: { state: "Unreachable" } },
  };
  assert.equal(providerNote("Ollama", down), "not running");
  assert.equal(providerNote("Other", status), "settings.toml");
});

test("the chosen model's panel says who serves it, how it thinks and if it draws", () => {
  const status = settings();
  assert.equal(servedBy(status, []), "Cerebras only");
  assert.equal(servedBy(status, RECORDED_QWEN_PROVIDERS), "Cerebras only");
  const unrouted = { ...status, talker: { ...status.talker, route: [] } };
  assert.equal(servedBy(unrouted, []), "OpenRouter picks");
  assert.equal(servedBy(firstLaunch(), []), "Anthropic");
  assert.equal(thinkingLabel("Off"), "Off for speech");
  assert.equal(thinkingLabel("Low"), "Low");
  assert.equal(thinkingLabel("ExtraHigh"), "Extra high");
  assert.equal(drawsDiagrams(status.talker.drawing), "Checked ✓ (11 of 12)");
  assert.equal(drawsDiagrams(firstLaunch().talker.drawing), "Not measured");
  const haiku = RECORDED_MODELS.find((model) => model.id === "anthropic/claude-haiku-5.5");
  assert.equal(drawsDiagrams(haiku?.drawing ?? { kind: "NotMeasured" }), "Sometimes (5 of 12)");
  assert.equal(scribeLine(status), "Memory digests use qwen3.8-27b");
  assert.equal(scribeChangeable("Claude"), false);
  assert.equal(scribeChangeable("OpenRouter"), true);
  const cerebras = RECORDED_QWEN_PROVIDERS.find((provider) => provider.name === "Cerebras");
  assert.deepEqual(cerebras === undefined ? null : providerRow(cerebras, ["cerebras"]), {
    name: "Cerebras",
    detail: "$0.99 / $1.49 · 65K",
    routed: true,
  });
});

test("a missing talker key is named, and a key not checked never is", () => {
  const status = settings();
  assert.equal(talkerKeyMissing(status), null);
  const missing = { ...status, talker: { ...status.talker, key: "Missing" as const } };
  assert.deepEqual(talkerKeyMissing(missing), {
    key: "OpenRouter",
    words: "Talking needs your OpenRouter API key.",
  });
  const locked = { ...status, talker: { ...status.talker, key: "Unreadable" as const } };
  assert.match(talkerKeyMissing(locked)?.words ?? "", /Unlock it/);
  const unchecked = { ...status, talker: { ...status.talker, key: "NotChecked" as const } };
  assert.equal(talkerKeyMissing(unchecked), null);
});

test("the microphone's permission names whose switch it is (D-149)", () => {
  const status = settings();
  assert.deepEqual(permissionLine(status), {
    tone: "ok",
    text: "Allowed · asked as Visual Studio Code",
  });
  assert.equal(permissionHelp(status), null);
  const own = { ...status, mic_attributed_to: null };
  assert.equal(permissionLine(own).text, "Allowed");
  const denied = { ...status, microphone: "Denied" as const };
  assert.equal(permissionLine(denied).tone, "off");
  assert.match(permissionHelp(denied) ?? "", /Turn on Visual Studio Code/);
  assert.deepEqual(permissionLine({ ...status, microphone: "Undetermined" }), {
    tone: "quiet",
    text: "Not asked yet",
  });
});

test("inputs follow macOS first, the built-in by its name, and a gone device stays listed", () => {
  const status = settings();
  assert.deepEqual(
    inputRows(status).map((row) => [row.name, row.note]),
    [
      ["System default", "Follows macOS · now Shure MV7+"],
      ["MacBook Pro Microphone", "Built in"],
      ["Shure MV7+", ""],
      ["Studio Display Microphone", ""],
    ],
  );
  const unplugged: SetupStatus = {
    ...status,
    input: { kind: "Named", name: "Jabra Evolve2" },
    default_input: null,
  };
  const rows = inputRows(unplugged);
  assert.equal(rows[0]?.note, "Follows macOS");
  assert.deepEqual(rows.at(-1), {
    device: { kind: "Named", name: "Jabra Evolve2" },
    name: "Jabra Evolve2",
    note: "Not connected · the system default stands in",
  });
  assert.equal(sectionBadge("microphone", status), "Shure MV7+");
});

test("voice cards say the download, the key ElevenLabs needs, and its voice (D-137)", () => {
  const status = settings();
  const kokoro = groupProgress(status.voice_files, new Map());
  assert.equal(voiceStatus("Kokoro", status, kokoro), "ready · 312 MB");
  const fresh = firstLaunch();
  const missing = groupProgress(fresh.voice_files, new Map());
  assert.equal(voiceStatus("Kokoro", fresh, missing), "312 MB to download");
  const stopped = { ...missing, problem: "no connection" };
  assert.equal(voiceStatus("Kokoro", fresh, stopped), "download stopped");
  assert.equal(voiceStatus("ElevenLabs", status, kokoro), "Flash · Rachel");
  assert.equal(elevenLabsPickable(status), true);
  const noKey = withKey(status, "ElevenLabs", "Missing");
  assert.equal(voiceStatus("ElevenLabs", noKey, kokoro), "needs a key");
  assert.equal(elevenLabsPickable(noKey), false);
  const unchecked = withKey(status, "ElevenLabs", "NotChecked");
  assert.equal(voiceStatus("ElevenLabs", unchecked, kokoro), "Flash · Rachel · key not checked");
  assert.equal(elevenLabsPickable(unchecked), true, "the app checks the key when it is picked");
  assert.equal(voiceStatus("System", status, kokoro), "always available");
});

test("a harness reads found with its version and default, or not installed", () => {
  assert.equal(
    harnessLine({ state: "Found", version: "2.1.301" }, true),
    "found · 2.1.301 · default",
  );
  assert.equal(harnessLine({ state: "Found", version: null }, false), "found");
  assert.equal(harnessLine({ state: "Missing" }, false), "not installed");
});

test("the nav's badges come from the status", () => {
  const status = settings();
  assert.equal(sectionBadge("thinking", status), "gpt-oss-120b");
  assert.equal(sectionBadge("voice", status), "Kokoro");
  assert.equal(sectionBadge("keys", status), "2 saved");
  assert.equal(
    sectionBadge("keys", { ...status, keys: keyStates([], "NotChecked") }),
    "not checked",
  );
  assert.equal(sectionBadge("coding", status), "");
});

test("the section being read is the last one scrolled up to the top", () => {
  assert.equal(activeSection([]), "thinking");
  assert.equal(
    activeSection([
      { section: "thinking", top: -800 },
      { section: "voice", top: -40 },
      { section: "microphone", top: 300 },
    ]),
    "voice",
  );
  const bottom = [
    { section: "keys" as const, top: -200 },
    { section: "coding" as const, top: 520 },
  ];
  assert.equal(activeSection(bottom, true, "coding"), "coding", "picked, at the page's end");
  assert.equal(activeSection(bottom, false, "coding"), "keys");
  assert.equal(activeSection(bottom, true, "thinking"), "keys", "a section above is scrolled past");
});

test("a rebuild says how many files it read and what it left", () => {
  assert.equal(
    rebuiltLine({ files_seen: 128, reindexed: 128, skipped: 0, truncated: false }),
    "Rebuilt from 128 files.",
  );
  assert.equal(
    rebuiltLine({ files_seen: 1, reindexed: 1, skipped: 2, truncated: true }),
    "Rebuilt from 1 file. 2 couldn't be read; the next scan tries them again. It stopped at its limit, so later files aren't indexed.",
  );
});
