// How the first-run screen reaches the app (crates/app/src/setup/commands.rs). Components never
// see Tauri: they see a controller over this interface, whose implementations are Tauri
// (tauriSetup.ts) and a fake (fake.ts) for tests and the visual check.
//
// A key goes out through `saveKey` and nothing ever comes back with it: the status only says
// which key names are saved.

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
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
   * turned on (D-186); rejects in plain words when it couldn't.
   */
  openMicrophoneSettings(): Promise<void>;
  /** Sets the setup-complete flag and starts the session; rejects while anything blocks. */
  finish(): Promise<void>;
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

const ARRAYS = ["keys", "voice_files", "hearing_files", "input_devices", "blockers"] as const;
const STRINGS = ["voice", "hearing_model", "microphone", "mic_mode"] as const;

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
  const talker = value.talker;
  if (!isRecord(talker) || typeof talker.provider !== "string") {
    return null;
  }
  if (typeof talker.model !== "string" || typeof talker.key !== "string") {
    return null;
  }
  if (!isRecord(talker.ollama) || !isRecord(value.input) || !isRecord(value.harnesses)) {
    return null;
  }
  // A shell from before the field (D-186) sends none: macOS then asks about BoardAI itself.
  const attributed = value.mic_attributed_to;
  const mic_attributed_to =
    typeof attributed === "string" && attributed.trim() !== "" ? attributed : null;
  return { ...value, mic_attributed_to } as unknown as SetupStatus;
}
