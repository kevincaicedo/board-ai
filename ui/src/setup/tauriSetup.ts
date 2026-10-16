// The first-run screen over Tauri IPC, through the same injectable bridge as the session
// transport (transport/tauri.ts stays the only module that imports Tauri).
//
//   invoke("setup_status")                          → SetupStatus
//   invoke("setup_save", { edit })                  → SetupStatus
//   invoke("setup_key", { name, value })            → SetupStatus (never the key)
//   invoke("setup_download", { model, progress })   → resolves when verified; events on the channel
//   invoke("setup_microphone")                      → SetupStatus
//   invoke("setup_finish")                          → starts the session

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { SetupEdit } from "../protocol/generated/SetupEdit.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TauriBridge } from "../transport/tauri.ts";
import { parseSetupStatus, reason, type SetupTransport } from "./transport.ts";

const MISMATCH = "BoardAI's window and app don't match. Reinstall BoardAI.";

const DOWNLOAD_KINDS = new Set(["Progress", "Done", "Failed"]);

function isDownloadEvent(message: unknown): message is DownloadEvent {
  return (
    typeof message === "object" &&
    message !== null &&
    "kind" in message &&
    typeof message.kind === "string" &&
    DOWNLOAD_KINDS.has(message.kind)
  );
}

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
      if (isDownloadEvent(message)) {
        onEvent(message);
      } else {
        this.malformedEvents += 1;
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

  async finish(): Promise<void> {
    try {
      await this.#bridge.invoke("setup_finish", {});
    } catch (error) {
      throw new Error(reason(error));
    }
  }

  async #status(
    command: "setup_status" | "setup_save" | "setup_key" | "setup_microphone",
    args: Record<string, unknown>,
  ): Promise<SetupStatus> {
    let answer: unknown;
    try {
      answer = await this.#bridge.invoke(command, args);
    } catch (error) {
      throw new Error(reason(error));
    }
    const status = parseSetupStatus(answer);
    if (status === null) {
      throw new Error(MISMATCH);
    }
    return status;
  }
}
