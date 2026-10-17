// How the first-run screen and the settings page reach the app (crates/app/src/setup/commands.rs).
// Components never see Tauri: they see a controller over this interface, whose implementations
// are Tauri (tauriSetup.ts) and a fake (fake.ts) for tests and the visual check.
//
// A key goes out through `saveKey` and nothing ever comes back with it: the status only says
// which key names are saved. No path or URL goes out: a code folder is picked in the system's
// chooser, which the app shows (`addCodeFolder`), and removed by its place in the list. What
// comes back is parsed at the boundary in parse.ts.

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
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
