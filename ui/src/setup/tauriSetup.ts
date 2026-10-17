// The first-run screen and the settings page over Tauri IPC, through the same injectable bridge
// as the session transport (transport/tauri.ts stays the only module that imports Tauri).
//
//   invoke("setup_status")                          → SetupStatus
//   invoke("setup_save", { edit })                  → SetupStatus
//   invoke("setup_key", { name, value })            → SetupStatus (never the key)
//   invoke("setup_download", { model, progress })   → resolves when verified; events on the channel
//   invoke("setup_microphone")                      → SetupStatus
//   invoke("setup_open_microphone_settings")        → opens System Settings at the microphone
//   invoke("setup_finish")                          → starts the session
//   invoke("setup_models", { query })               → ModelListing
//   invoke("setup_remove_key", { name })            → SetupStatus
//   invoke("setup_check_keys")                      → SetupStatus
//   invoke("setup_harness_versions")                → SetupStatus
//   invoke("setup_add_code_folder")                 → SetupStatus (the app shows the chooser)
//   invoke("setup_open_vault")                      → opens the vault in Finder
//   invoke("setup_rebuild_index")                   → IndexRebuilt

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelQuery } from "../protocol/generated/ModelQuery.ts";
import type { SetupEdit } from "../protocol/generated/SetupEdit.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TauriBridge, TauriCommand } from "../transport/tauri.ts";
import {
  parseDownloadEvent,
  parseIndexRebuilt,
  parseModelListing,
  parseSetupStatus,
} from "./parse.ts";
import { reason, type SetupTransport } from "./transport.ts";

const MISMATCH = "BoardAI's window and app don't match. Reinstall BoardAI.";

export class TauriSetupTransport implements SetupTransport {
  readonly #bridge: TauriBridge;
  /** Download events that didn't parse (a mismatched build); dropped. */
  malformedEvents = 0;

  constructor(bridge: TauriBridge) {
    this.#bridge = bridge;
  }

  status(): Promise<SetupStatus> {
    return this.#status("setup_status", {});
  }

  save(edit: SetupEdit): Promise<SetupStatus> {
    return this.#status("setup_save", { edit });
  }

  saveKey(name: KeyName, value: string): Promise<SetupStatus> {
    return this.#status("setup_key", { name, value });
  }

  async download(model: string, onEvent: (event: DownloadEvent) => void): Promise<void> {
    const progress = this.#bridge.channel((message) => {
      const event = parseDownloadEvent(message);
      if (event === null) {
        this.malformedEvents += 1;
      } else {
        onEvent(event);
      }
    });
    try {
      await this.#bridge.invoke("setup_download", { model, progress });
    } catch (error) {
      throw new Error(reason(error));
    }
  }

  requestMicrophone(): Promise<SetupStatus> {
    return this.#status("setup_microphone", {});
  }

  async openMicrophoneSettings(): Promise<void> {
    await this.#call("setup_open_microphone_settings", {});
  }

  async finish(): Promise<void> {
    await this.#call("setup_finish", {});
  }

  async models(query: ModelQuery): Promise<ModelListing> {
    const listing = parseModelListing(await this.#call("setup_models", { query }));
    if (listing === null) {
      throw new Error(MISMATCH);
    }
    return listing;
  }

  removeKey(name: KeyName): Promise<SetupStatus> {
    return this.#status("setup_remove_key", { name });
  }

  checkKeys(): Promise<SetupStatus> {
    return this.#status("setup_check_keys", {});
  }

  harnessVersions(): Promise<SetupStatus> {
    return this.#status("setup_harness_versions", {});
  }

  addCodeFolder(): Promise<SetupStatus> {
    return this.#status("setup_add_code_folder", {});
  }

  async openVault(): Promise<void> {
    await this.#call("setup_open_vault", {});
  }

  async rebuildIndex(): Promise<IndexRebuilt> {
    const rebuilt = parseIndexRebuilt(await this.#call("setup_rebuild_index", {}));
    if (rebuilt === null) {
      throw new Error(MISMATCH);
    }
    return rebuilt;
  }

  async #call(command: TauriCommand, args: Record<string, unknown>): Promise<unknown> {
    try {
      return await this.#bridge.invoke(command, args);
    } catch (error) {
      throw new Error(reason(error));
    }
  }

  async #status(command: TauriCommand, args: Record<string, unknown>): Promise<SetupStatus> {
    const status = parseSetupStatus(await this.#call(command, args));
    if (status === null) {
      throw new Error(MISMATCH);
    }
    return status;
  }
}
