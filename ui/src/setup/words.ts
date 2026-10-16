// Everything the first-run screen says, in plain words (artboard 11 · First run). Kept apart
// from the components so the wording is tested and the components only lay it out.

import type { DownloadProblem } from "../protocol/generated/DownloadProblem.ts";
import type { InputDevice } from "../protocol/generated/InputDevice.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { MicChoice } from "../protocol/generated/MicChoice.ts";
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
      return "ElevenLabs API key";
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
      return "sk_…";
  }
}

export const KEY_NOTE = "Saved to Keychain. Never written to a file or a log.";

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
      return "Default input";
    case "BuiltIn":
      return "Built-in mic";
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
      return "The download stopped. Check your connection and try again.";
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
  "Turn the microphone on in System Settings › Privacy & Security › Microphone, then come back.";

export const OLLAMA_UNREACHABLE =
  "Ollama isn't answering. Start it, then come back to this window.";

/** Why "Start talking" waits: the first blocker, in plain words. */
export function startHint(status: SetupStatus): string | null {
  const blocker: SetupBlocker | undefined = status.blockers[0];
  if (blocker === undefined) {
    return null;
  }
  switch (blocker.kind) {
    case "SettingsProblem":
      return `settings.toml can't be used: ${status.settings_problem ?? "it doesn't parse"}. Fix it or move it away.`;
    case "TalkerKey": {
      const key = providerKey(blocker.provider);
      return key === null
        ? "Save the key your settings.toml names."
        : `Paste your ${keyLabel(key)} to start.`;
    }
    case "OllamaUnreachable":
      return "Start Ollama, or pick another provider.";
    case "OllamaModelMissing":
      return `Pull ${blocker.model} in Ollama, or pick another model.`;
    case "UnknownHearingModel":
      return `settings.toml asks for Whisper ${blocker.model}, which BoardAI doesn't have.`;
    case "HearingNotReady":
      return "Starts once the hearing model is ready.";
    case "MicrophoneNotAllowed":
      return "Allow the microphone to start.";
    case "MicrophoneDenied":
      return "Turn the microphone on in System Settings to start.";
  }
}
