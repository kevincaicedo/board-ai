// Everything the first-run screen says, in plain words (artboards 11 · First run and 11b · First
// run states). Kept apart from the components so the wording is tested and the components only
// lay it out.

import type { DownloadProblem } from "../protocol/generated/DownloadProblem.ts";
import type { InputDevice } from "../protocol/generated/InputDevice.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { MicChoice } from "../protocol/generated/MicChoice.ts";
import type { MicPermission } from "../protocol/generated/MicPermission.ts";
import type { SetupBlocker } from "../protocol/generated/SetupBlocker.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TalkerProvider } from "../protocol/generated/TalkerProvider.ts";
import type { GroupProgress } from "./state.ts";

/** Sizes as the design shows them ("201 of 312 MB"): binary megabytes, whole. */
const BYTES_PER_MEGABYTE = 1024 * 1024;

export function megabytes(bytes: number): number {
  return Math.round(bytes / BYTES_PER_MEGABYTE);
}

/** The right side of a model row: "312 MB", "201 of 312 MB" or "ready · 142 MB". */
export function groupSize(progress: GroupProgress): string {
  const total = megabytes(progress.totalBytes);
  if (progress.ready) {
    return `ready · ${total} MB`;
  }
  if (progress.receivedBytes > 0) {
    return `${megabytes(progress.receivedBytes)} of ${total} MB`;
  }
  return `${total} MB`;
}

/** 0 to 100, for the bar and its aria value. */
export function percent(progress: GroupProgress): number {
  if (progress.ready) {
    return 100;
  }
  if (progress.totalBytes <= 0) {
    return 0;
  }
  return Math.min(100, Math.floor((progress.receivedBytes * 100) / progress.totalBytes));
}

export const PROVIDERS: readonly { readonly id: TalkerProvider; readonly label: string }[] = [
  { id: "Claude", label: "Claude" },
  { id: "OpenRouter", label: "OpenRouter" },
  { id: "Ollama", label: "Ollama · local" },
];

/** The key a provider's field saves; `null` for providers without one. */
export function providerKey(provider: TalkerProvider): KeyName | null {
  switch (provider) {
    case "Claude":
      return "Anthropic";
    case "OpenRouter":
      return "OpenRouter";
    case "Ollama":
    case "Other":
      return null;
  }
}

export function keyLabel(key: KeyName): string {
  switch (key) {
    case "Anthropic":
      return "Anthropic API key";
    case "OpenRouter":
      return "OpenRouter API key";
    case "ElevenLabs":
      return "ElevenLabs API key (optional voice)";
  }
}

/** The empty field's hint; once a key is saved it says so instead (the key never comes back). */
export function keyHint(key: KeyName, saved: boolean): string {
  if (saved) {
    return "Saved · paste to replace";
  }
  switch (key) {
    case "Anthropic":
      return "sk-ant-…";
    case "OpenRouter":
      return "sk-or-…";
    case "ElevenLabs":
      return "Paste, then press Return";
  }
}

export const KEY_NOTE = "Saved to Keychain. Never written to a file or a log.";

/** Under a key field: where a key goes, or, once one is saved, that it never comes back. */
export function keyNote(saved: boolean): string {
  return saved ? "The key never comes back to this screen: only that one is saved." : KEY_NOTE;
}

/** A card's step line, with its state when it has one ("3 · MICROPHONE · DENIED", 11b). */
function step(base: string, state: string | null): string {
  return state === null ? base : `${base} · ${state}`;
}

export function thinkingStep(status: SetupStatus): string {
  return step("1 · THINKING", status.talker.key === "Saved" ? "KEY SAVED" : null);
}

export function voiceStep(stopped: boolean): string {
  return step("2 · VOICE AND HEARING", stopped ? "ERROR" : null);
}

export function micStep(permission: MicPermission): string {
  return step("3 · MICROPHONE", permission === "Denied" ? "DENIED" : null);
}

/** The line under the cards on the first run (11b). */
export const FOOTER_FIRST_RUN = "Reopen any time with ⌘, or by saying “open settings”.";
/** The same line over a running session, where the screen is the settings. */
export const FOOTER_SETTINGS = "Esc closes settings. A change starts a new session.";

/** The button under the cards: it starts the session, or returns to the running one. */
export function startLabel(settings: boolean): string {
  return settings ? "Back to talking" : "Start talking";
}

export const MODES: readonly {
  readonly id: MicChoice;
  readonly label: string;
  readonly note: string;
}[] = [
  { id: "Open", label: "Open mic", note: "Just talk. Best with headphones." },
  { id: "PushToTalk", label: "Push to talk", note: "Hold Space while you speak." },
];

export function inputLabel(device: InputDevice): string {
  switch (device.kind) {
    case "SystemDefault":
      return "System default";
    case "BuiltIn":
      return "Built-in microphone";
    case "Named":
      return device.name;
  }
}

export function harnessChip(name: string, found: boolean): string {
  return `${name} · ${found ? "found" : "not installed"}`;
}

export function downloadProblem(problem: DownloadProblem): string {
  switch (problem.kind) {
    case "Network":
      return "The download stopped: no connection. It picks up where it left off.";
    case "Server":
      return `The model server answered ${problem.status}. Try again in a while.`;
    case "Unexpected":
      return "The model server sent something unexpected. Try again.";
    case "Corrupt":
      return "The file didn't match its checksum, so it was deleted. Try again.";
    case "Disk":
      return "BoardAI couldn't save the file. Check your free space and try again.";
  }
}

export const MIC_DENIED =
  "Turn the microphone on in System Settings › Privacy & Security › Microphone, then come back. This screen notices on its own.";

/**
 * Where to turn the microphone on when macOS said no. Started from a terminal or an editor,
 * BoardAI's microphone is that app's, so the switch to turn on is that app's (D-186).
 */
export function micDenied(attributedTo: string | null): string {
  if (attributedTo === null) {
    return MIC_DENIED;
  }
  return `macOS asks about ${attributedTo} here, because BoardAI was started from it. Turn on ${attributedTo} in System Settings › Privacy & Security › Microphone, then come back.`;
}

/** The button under a denied microphone's words: System Settings at its pane. */
export const OPEN_MIC_SETTINGS = "Open Microphone settings";

export const OLLAMA_UNREACHABLE =
  "Ollama isn't answering. Start it, then come back to this window.";

/** What one blocker waits for, said after "Waiting for:". */
function waitingFor(blocker: SetupBlocker, status: SetupStatus): string {
  switch (blocker.kind) {
    case "SettingsProblem":
      return `a settings.toml BoardAI can read (${status.settings_problem ?? "it doesn't parse"}; fix it or move it away)`;
    case "TalkerKey": {
      const key = providerKey(blocker.provider);
      return key === null ? "the key settings.toml names" : `your ${keyLabel(key)}`;
    }
    case "OllamaUnreachable":
      return "Ollama to start";
    case "OllamaModelMissing":
      return `${blocker.model} in Ollama`;
    case "UnknownHearingModel":
      return `a hearing model BoardAI has (settings.toml asks for Whisper ${blocker.model})`;
    case "HearingNotReady":
      return "the hearing model";
    case "MicrophoneNotAllowed":
    case "MicrophoneDenied":
      return "the microphone";
  }
}

/**
 * Why "Start talking" waits (11b): every blocker, each named once, in one line — "Waiting for:
 * the hearing model, and the microphone." The cards say how to fix each one.
 */
export function startHint(status: SetupStatus): string | null {
  const names: string[] = [];
  for (const blocker of status.blockers) {
    const name = waitingFor(blocker, status);
    if (!names.includes(name)) {
      names.push(name);
    }
  }
  if (names.length === 0) {
    return null;
  }
  const last = names.pop() ?? "";
  return `Waiting for: ${names.length === 0 ? last : `${names.join(", ")}, and ${last}`}.`;
}
