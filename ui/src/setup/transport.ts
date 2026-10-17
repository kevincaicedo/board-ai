// How the first-run screen and the settings page reach the app (crates/app/src/setup/commands.rs).
// Components never see Tauri: they see a controller over this interface, whose implementations
// are Tauri (tauriSetup.ts) and a fake (fake.ts) for tests and the visual check.
//
// A key goes out through `saveKey` and nothing ever comes back with it: the status only says
// which key names are saved. No path or URL goes out: a code folder is picked in the system's
// chooser, which the app shows (`addCodeFolder`), and removed by its place in the list.

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { ModelInfo } from "../protocol/generated/ModelInfo.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { ModelQuery } from "../protocol/generated/ModelQuery.ts";
import type { SetupEdit } from "../protocol/generated/SetupEdit.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";

export interface SetupTransport {
  status(): Promise<SetupStatus>;
  /** Saves one choice to settings.toml; resolves with the fresh status. */
  save(edit: SetupEdit): Promise<SetupStatus>;
  /** Saves a pasted key to the Keychain under its name; resolves with the fresh status. */
  saveKey(name: KeyName, value: string): Promise<SetupStatus>;
  /** Downloads one model file; `onEvent` gets throttled progress, then Done or Failed. */
  download(model: string, onEvent: (event: DownloadEvent) => void): Promise<void>;
  requestMicrophone(): Promise<SetupStatus>;
  /**
   * Opens System Settings at Privacy & Security › Microphone, where the app macOS asks about is
   * turned on (D-149); rejects in plain words when it couldn't.
   */
  openMicrophoneSettings(): Promise<void>;
  /** Sets the setup-complete flag and starts the session; rejects while anything blocks. */
  finish(): Promise<void>;
  /** The model picker's lists (D-153): bounded, cached by the app, no key needed. */
  models(query: ModelQuery): Promise<ModelListing>;
  /** Deletes a key from the Keychain; resolves with the fresh status. */
  removeKey(name: KeyName): Promise<SetupStatus>;
  /** Reads the Keychain for every key now (the owner asked; an unsigned build may prompt). */
  checkKeys(): Promise<SetupStatus>;
  /** Asks each installed harness for its version once; resolves with the fresh status. */
  harnessVersions(): Promise<SetupStatus>;
  /** Shows the macOS folder chooser; a folder picked there is added. Closing it adds nothing. */
  addCodeFolder(): Promise<SetupStatus>;
  /** Opens the vault in Finder. */
  openVault(): Promise<void>;
  /** Empties the index and fills it again from the vault. */
  rebuildIndex(): Promise<IndexRebuilt>;
}

/** A rejection in plain words: the app's own message, or a generic line. */
export function reason(error: unknown): string {
  if (typeof error === "string" && error.trim() !== "") {
    return error;
  }
  if (error instanceof Error && error.message.trim() !== "") {
    return error.message;
  }
  return "Something went wrong. Try again.";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const ARRAYS = [
  "keys",
  "voice_files",
  "hearing_files",
  "input_devices",
  "blockers",
  "code_folders",
] as const;
const STRINGS = [
  "voice",
  "hearing_model",
  "microphone",
  "mic_mode",
  "scribe_model",
  "vault",
] as const;

/**
 * The status as the app sent it, checked at the boundary: a shell from another build must
 * not crash the screen. The shape is the generated type's; this checks the fields the screen
 * branches on.
 */
export function parseSetupStatus(value: unknown): SetupStatus | null {
  if (!isRecord(value) || typeof value.complete !== "boolean") {
    return null;
  }
  if (value.settings_problem !== null && typeof value.settings_problem !== "string") {
    return null;
  }
  if (ARRAYS.some((name) => !Array.isArray(value[name]))) {
    return null;
  }
  if (STRINGS.some((name) => typeof value[name] !== "string")) {
    return null;
  }
  const keys = value.keys;
  if (
    !Array.isArray(keys) ||
    !keys.every((key) => isRecord(key) && typeof key.state === "string")
  ) {
    return null;
  }
  const talker = value.talker;
  if (!isRecord(talker) || typeof talker.provider !== "string") {
    return null;
  }
  if (typeof talker.model !== "string" || typeof talker.key !== "string") {
    return null;
  }
  if (typeof talker.thinking !== "string" || !Array.isArray(talker.route)) {
    return null;
  }
  if (!isRecord(talker.ollama) || !isRecord(value.input) || !isRecord(value.harnesses)) {
    return null;
  }
  if (!isRecord(value.elevenlabs)) {
    return null;
  }
  // A shell from before the field (D-149) sends none: macOS then asks about BoardAI itself.
  const attributed = value.mic_attributed_to;
  const mic_attributed_to =
    typeof attributed === "string" && attributed.trim() !== "" ? attributed : null;
  return { ...value, mic_attributed_to } as unknown as SetupStatus;
}

/** A model row as the app sent it, or null; a bad row is left out, the rest still show. */
function parseModel(value: unknown): ModelInfo | null {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.name !== "string") {
    return null;
  }
  if (typeof value.preset !== "boolean" || !isRecord(value.reasoning)) {
    return null;
  }
  const route = Array.isArray(value.route)
    ? value.route.filter((slug): slug is string => typeof slug === "string")
    : [];
  return { ...value, route } as unknown as ModelInfo;
}

function parseProvider(value: unknown): ModelProvider | null {
  if (!isRecord(value) || typeof value.name !== "string" || typeof value.slug !== "string") {
    return null;
  }
  const number = (field: unknown): number | null =>
    typeof field === "number" && Number.isFinite(field) && field >= 0 ? field : null;
  return {
    ...value,
    tools: value.tools === true,
    latency_ms_p50: number(value.latency_ms_p50),
    throughput_tokens_per_second_p50: number(value.throughput_tokens_per_second_p50),
  } as unknown as ModelProvider;
}

function rows<T>(values: unknown, parse: (value: unknown) => T | null): T[] {
  if (!Array.isArray(values)) {
    return [];
  }
  return values.map(parse).filter((row): row is T => row !== null);
}

/**
 * A model list as the app sent it, checked at the boundary like the status: its kind and
 * arrays, then each row on its own (a row that doesn't parse is skipped, as in the views,
 * D-119). A speed is a number or unknown, never made up.
 */
export function parseModelListing(value: unknown): ModelListing | null {
  if (!isRecord(value) || typeof value.kind !== "string") {
    return null;
  }
  const leftOut = typeof value.left_out === "number" ? value.left_out : 0;
  switch (value.kind) {
    case "OpenRouter":
      return Array.isArray(value.models)
        ? { kind: "OpenRouter", models: rows(value.models, parseModel), left_out: leftOut }
        : null;
    case "Ollama":
      return Array.isArray(value.models)
        ? { kind: "Ollama", models: rows(value.models, parseModel) }
        : null;
    case "Providers":
      return typeof value.model === "string" && Array.isArray(value.providers)
        ? {
            kind: "Providers",
            model: value.model,
            providers: rows(value.providers, parseProvider),
            left_out: leftOut,
          }
        : null;
    default:
      return null;
  }
}

/** What "Rebuild the index" answered, or null. */
export function parseIndexRebuilt(value: unknown): IndexRebuilt | null {
  if (!isRecord(value) || typeof value.truncated !== "boolean") {
    return null;
  }
  const counts = [value.files_seen, value.reindexed, value.skipped];
  if (!counts.every((count) => typeof count === "number" && count >= 0)) {
    return null;
  }
  return value as unknown as IndexRebuilt;
}
