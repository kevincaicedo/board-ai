// Everything the settings page says (artboard 12 · Settings), in plain words, apart from the
// components so the wording is tested and the components only lay it out. Every value comes
// from the app's status or its lists; nothing here is a number the app didn't send.

import type { Drawing } from "../protocol/generated/Drawing.ts";
import type { HarnessFound } from "../protocol/generated/HarnessFound.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { InputDevice } from "../protocol/generated/InputDevice.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { KeyState } from "../protocol/generated/KeyState.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TalkerProvider } from "../protocol/generated/TalkerProvider.ts";
import type { ThinkingEffort } from "../protocol/generated/ThinkingEffort.ts";
import type { VoiceEngine } from "../protocol/generated/VoiceEngine.ts";
import {
  contextLabel,
  latencyLabel,
  priceLabel,
  routeLabel,
  slugBase,
  speedLabel,
} from "./models.ts";
import type { GroupProgress } from "./state.ts";
import { groupSize, keyLabel, micDenied, providerKey } from "./words.ts";

export type Section = "thinking" | "voice" | "microphone" | "keys" | "coding" | "memory";

export const SECTIONS: readonly { readonly id: Section; readonly label: string }[] = [
  { id: "thinking", label: "Thinking" },
  { id: "voice", label: "Voice" },
  { id: "microphone", label: "Microphone" },
  { id: "keys", label: "Keys" },
  { id: "coding", label: "Coding" },
  { id: "memory", label: "Memory and privacy" },
];

export const SETTINGS_TITLE = "Everything BoardAI uses, in one place";

/** Every setting is read when a session starts, so any change starts a fresh one. */
export const SETTINGS_LEDE =
  "Changes apply when you go back to talking, in a fresh session. Keys stay in your macOS Keychain.";

export const SETTINGS_HINT = "Say “open settings” or press ⌘, from the board. Esc goes back.";

export const BACK_TO_TALKING = "Back to talking";

/** A section counts as the one being read once its top is this far below the page's top. */
export const ACTIVE_SECTION_OFFSET_PX = 120;

/**
 * The section being read, for the nav: the last one whose top has scrolled up to the offset.
 * At the end of the page the last sections can't scroll that far, so one picked in the nav
 * below the rule's answer stays the one being read.
 */
export function activeSection(
  tops: readonly { section: Section; top: number }[],
  atEnd = false,
  picked: Section | null = null,
): Section {
  let active: Section = "thinking";
  for (const { section, top } of tops) {
    if (top <= ACTIVE_SECTION_OFFSET_PX) {
      active = section;
    }
  }
  const order = SECTIONS.map(({ id }) => id);
  if (atEnd && picked !== null && order.indexOf(picked) > order.indexOf(active)) {
    return picked;
  }
  return active;
}

/** The talker's model as the nav shows it: the part after its author. */
export function shortModel(model: string): string {
  const slash = model.lastIndexOf("/");
  return slash < 0 ? model : model.slice(slash + 1);
}

export function keyStateOf(status: SetupStatus, key: KeyName): KeyState {
  return status.keys.find((entry) => entry.name === key)?.state ?? "NotChecked";
}

export function voiceName(engine: VoiceEngine): string {
  switch (engine) {
    case "Kokoro":
      return "Kokoro";
    case "ElevenLabs":
      return "ElevenLabs";
    case "System":
      return "macOS voice";
  }
}

/** The input as the nav and the list name it. */
export function inputName(device: InputDevice, status: SetupStatus): string {
  switch (device.kind) {
    case "SystemDefault":
      return "System default";
    case "BuiltIn":
      return status.built_in_input ?? "Built-in microphone";
    case "Named":
      return device.name;
  }
}

/** The nav's note beside each section. */
export function sectionBadge(section: Section, status: SetupStatus): string {
  switch (section) {
    case "thinking":
      return shortModel(status.talker.model);
    case "voice":
      return voiceName(status.voice);
    case "microphone":
      return status.input.kind === "SystemDefault"
        ? (status.default_input ?? "System default")
        : inputName(status.input, status);
    case "keys": {
      const saved = status.keys.filter((key) => key.state === "Saved").length;
      const unchecked = status.keys.some((key) => key.state === "NotChecked");
      return saved === 0 && unchecked ? "not checked" : `${saved} saved`;
    }
    case "coding":
    case "memory":
      return "";
  }
}

export const TALKER_PROVIDERS: readonly { readonly id: TalkerProvider; readonly label: string }[] =
  [
    { id: "Claude", label: "Claude" },
    { id: "OpenRouter", label: "OpenRouter" },
    { id: "Ollama", label: "Ollama · local" },
  ];

function keyNote(state: KeyState): string {
  switch (state) {
    case "Saved":
      return "key saved";
    case "Missing":
      return "key needed";
    case "Unreadable":
      return "Keychain locked";
    case "NotChecked":
      return "key not checked";
    case "NotNeeded":
      return "";
  }
}

/** The small note on a provider pill: its key, or Ollama's state. */
export function providerNote(provider: TalkerProvider, status: SetupStatus): string {
  switch (provider) {
    case "Claude":
      return keyNote(keyStateOf(status, "Anthropic"));
    case "OpenRouter":
      return keyNote(keyStateOf(status, "OpenRouter"));
    case "Ollama": {
      const ollama = status.talker.ollama;
      if (ollama.state === "Unreachable") {
        return "not running";
      }
      const local = status.talker.provider === "Ollama" ? status.talker.model : ollama.models[0];
      return local === undefined ? "no models" : shortModel(local);
    }
    case "Other":
      return "settings.toml";
  }
}

export function thinkingLabel(thinking: ThinkingEffort): string {
  switch (thinking) {
    case "Off":
      return "Off for speech";
    case "Low":
      return "Low";
    case "Medium":
      return "Medium";
    case "High":
      return "High";
    case "ExtraHigh":
      return "Extra high";
    case "Max":
      return "Max";
  }
}

/** Who serves the talker, for the selected-model panel. */
export function servedBy(status: SetupStatus, providers: readonly ModelProvider[]): string {
  switch (status.talker.provider) {
    case "OpenRouter":
      return routeLabel(status.talker.route, providers);
    case "Ollama":
      return "This Mac";
    case "Claude":
      return "Anthropic";
    case "Other":
      return "The server settings.toml names";
  }
}

/**
 * How it draws diagrams, as spike S5 measured it (D-219). The core judged it (checked from 10 of
 * every 12 answers); the panel says how many drew, never more than was measured.
 */
export function drawsDiagrams(drawing: Drawing): string {
  switch (drawing.kind) {
    case "Checked":
      return `Checked ✓ (${drawing.measure.drew} of ${drawing.measure.answers})`;
    case "Sometimes":
      return `Sometimes (${drawing.measure.drew} of ${drawing.measure.answers})`;
    case "NotMeasured":
      return "Not measured";
  }
}

/** The talker's key is missing or can't be read: which key, and why it matters, in words. */
export function talkerKeyMissing(
  status: SetupStatus,
): { readonly key: KeyName; readonly words: string } | null {
  const key = providerKey(status.talker.provider);
  if (key === null) {
    return null;
  }
  switch (status.talker.key) {
    case "Missing":
      return { key, words: `Talking needs your ${keyLabel(key)}.` };
    case "Unreadable":
      return { key, words: "BoardAI couldn't read the Keychain. Unlock it, then check the keys." };
    case "Saved":
    case "NotChecked":
    case "NotNeeded":
      return null;
  }
}

/** Providers listed in the chosen model's panel; the rest are counted. */
export const PROVIDERS_SHOWN_MAX = 6;

/** A provider in the chosen model's panel: price, context and, when listed, its speed. */
export function providerRow(
  provider: ModelProvider,
  route: readonly string[],
): { readonly name: string; readonly detail: string; readonly routed: boolean } {
  const parts = [priceLabel(provider.price), contextLabel(provider.context_tokens)];
  if (provider.latency_ms_p50 !== null) {
    parts.push(latencyLabel(provider.latency_ms_p50));
  }
  if (provider.throughput_tokens_per_second_p50 !== null) {
    parts.push(speedLabel(provider.throughput_tokens_per_second_p50));
  }
  if (!provider.tools) {
    parts.push("no tools");
  }
  return {
    name: provider.name,
    detail: parts.join(" · "),
    routed: route.includes(slugBase(provider.slug)),
  };
}

export function scribeLine(status: SetupStatus): string {
  return `Memory digests use ${shortModel(status.scribe_model)}`;
}

/** Whether "change" shows beside the scribe: only where the page lists models to pick from. */
export function scribeChangeable(provider: TalkerProvider): boolean {
  return provider === "OpenRouter" || provider === "Ollama";
}

export function modelCount(
  count: number,
  leftOut: number,
  source: "OpenRouter" | "Ollama",
): string {
  const where = source === "OpenRouter" ? "with tools on OpenRouter" : "on this Mac";
  const left = leftOut > 0 ? ` (${leftOut} left out)` : "";
  return `Model · ${count} ${where}${left}`;
}

export const MODEL_SEARCH_HINT = "Search models: qwen, gpt-oss, free…";

export function hiddenRows(hidden: number): string {
  return `${hidden} more · search to narrow the list`;
}

export const NO_MATCH = "No model matches. Try fewer words.";

export const CLAUDE_MODEL_NOTE =
  "Claude uses its own models: Opus for talking, Haiku for memory digests.";

export const OTHER_PROVIDER_NOTE =
  "settings.toml names another server. Pick a provider above to replace it, or keep it.";

/** The voice cards' notes, from the canvas. */
export function voiceNote(engine: VoiceEngine): string {
  switch (engine) {
    case "Kokoro":
      return "On this Mac. Warm and unhurried.";
    case "ElevenLabs":
      return "Fastest and most natural. Needs a key.";
    case "System":
      return "No download. Used while others load.";
  }
}

/** The mono line under a voice card. */
export function voiceStatus(
  engine: VoiceEngine,
  status: SetupStatus,
  kokoro: GroupProgress,
): string {
  switch (engine) {
    case "Kokoro":
      if (kokoro.problem !== null) {
        return "download stopped";
      }
      return kokoro.ready || kokoro.receivedBytes > 0
        ? groupSize(kokoro)
        : `${groupSize(kokoro)} to download`;
    case "ElevenLabs":
      return elevenLabsStatus(status);
    case "System":
      return "always available";
  }
}

/** ElevenLabs' model and voice (D-137), or what its key needs. */
function elevenLabsStatus(status: SetupStatus): string {
  const named = `${status.elevenlabs.model} · ${status.elevenlabs.voice}`;
  switch (keyStateOf(status, "ElevenLabs")) {
    case "Saved":
    case "NotNeeded":
      return named;
    case "NotChecked":
      return `${named} · key not checked`;
    case "Missing":
      return "needs a key";
    case "Unreadable":
      return "Keychain locked";
  }
}

/** ElevenLabs can be picked once its key is saved (or not yet checked: the app checks). */
export function elevenLabsPickable(status: SetupStatus): boolean {
  const state = keyStateOf(status, "ElevenLabs");
  return state === "Saved" || state === "NotChecked";
}

export interface InputRow {
  readonly device: InputDevice;
  readonly name: string;
  readonly note: string;
}

/**
 * The input list (D-140): the system default first, following macOS; the Mac's own microphone
 * by its name; every other device macOS lists; and a chosen device that is gone, so the choice
 * still shows.
 */
export function inputRows(status: SetupStatus): InputRow[] {
  const builtIn = status.built_in_input;
  const rows: InputRow[] = [
    {
      device: { kind: "SystemDefault" },
      name: "System default",
      note:
        status.default_input === null
          ? "Follows macOS"
          : `Follows macOS · now ${status.default_input}`,
    },
    { device: { kind: "BuiltIn" }, name: builtIn ?? "Built-in microphone", note: "Built in" },
  ];
  for (const name of status.input_devices) {
    if (name !== builtIn) {
      rows.push({ device: { kind: "Named", name }, name, note: "" });
    }
  }
  const chosen = status.input;
  if (chosen.kind === "Named" && !status.input_devices.includes(chosen.name)) {
    rows.push({
      device: chosen,
      name: chosen.name,
      note: "Not connected · the system default stands in",
    });
  }
  return rows;
}

export function sameInput(left: InputDevice, right: InputDevice): boolean {
  switch (left.kind) {
    case "SystemDefault":
    case "BuiltIn":
      return right.kind === left.kind;
    case "Named":
      return right.kind === "Named" && right.name === left.name;
  }
}

export type Tone = "ok" | "off" | "quiet";

/** The permission row: a dot, and whose switch it is (D-149). */
export function permissionLine(status: SetupStatus): {
  readonly tone: Tone;
  readonly text: string;
} {
  const app = status.mic_attributed_to;
  switch (status.microphone) {
    case "Granted":
      return { tone: "ok", text: app === null ? "Allowed" : `Allowed · asked as ${app}` };
    case "Undetermined":
      return { tone: "quiet", text: "Not asked yet" };
    case "Denied":
      return { tone: "off", text: app === null ? "Off" : `Off · asked as ${app}` };
  }
}

/** Under a denied permission: where to turn it on. */
export function permissionHelp(status: SetupStatus): string | null {
  return status.microphone === "Denied" ? micDenied(status.mic_attributed_to) : null;
}

export const OPEN_MACOS_SETTINGS = "Open macOS settings";
export const ALLOW_MICROPHONE = "Allow microphone";

export const KEYS_NOTE = "In your macOS Keychain. BoardAI never shows a key again once saved.";

export interface KeyRow {
  readonly tone: Tone;
  readonly status: string;
  /** Opens the paste field. */
  readonly primary: string;
  readonly secondary: "Remove" | "Check" | null;
}

/** A row of the keys section, by the key's state. */
export function keyRow(state: KeyState): KeyRow {
  switch (state) {
    case "Saved":
      return { tone: "ok", status: "Saved", primary: "Replace", secondary: "Remove" };
    case "Missing":
    case "NotNeeded":
      return { tone: "quiet", status: "Not set", primary: "Add a key", secondary: null };
    case "NotChecked":
      return {
        tone: "quiet",
        status: "Not checked while talking",
        primary: "Replace",
        secondary: "Check",
      };
    case "Unreadable":
      return {
        tone: "off",
        status: "The Keychain is locked. Unlock it, then check again.",
        primary: "Replace",
        secondary: "Check",
      };
  }
}

export function keyHintFor(state: KeyState): string {
  return state === "Saved" ? "Saved · paste to replace" : "Paste, then press Return";
}

/** A harness row's right side: "found · 2.1.301 · default", "not installed". */
export function harnessLine(found: HarnessFound, isDefault: boolean): string {
  if (found.state === "Missing") {
    return "not installed";
  }
  const parts = ["found"];
  if (found.version !== null) {
    parts.push(found.version);
  }
  if (isDefault) {
    parts.push("default");
  }
  return parts.join(" · ");
}

export const NO_CODE_FOLDERS = "None yet. BoardAI reads only the folders you add here.";

/** What "Rebuild the index" did, in one line. */
export function rebuiltLine(rebuilt: IndexRebuilt): string {
  const files = rebuilt.files_seen === 1 ? "1 file" : `${rebuilt.files_seen} files`;
  const parts = [`Rebuilt from ${files}.`];
  if (rebuilt.skipped > 0) {
    parts.push(`${rebuilt.skipped} couldn't be read; the next scan tries them again.`);
  }
  if (rebuilt.truncated) {
    parts.push("It stopped at its limit, so later files aren't indexed.");
  }
  return parts.join(" ");
}
