// The live transport, over Tauri IPC. This module is the only place the UI touches Tauri.
//
// Contract with the app shell (crates/app implements the Rust side):
//
//   1. Once, at startup:   invoke("connect", { events: Channel<UiBatch> })
//      The shell keeps the channel and sends every UiBatch the bridge produces, in order, one
//      batch per frame at most (LLD §3.1 Bridge). The promise resolves once the channel is
//      registered; rejecting means there is no session to show.
//   2. Per command:        invoke("command", { command: UiCommand })
//      Fire and forget; a rejection is counted, never retried (a stale pointer target or mic
//      toggle is worse than a dropped one).
//   3. For a replay:       invoke("session_log", { id }) → string
//      A past session's events.jsonl by id (bounded by the shell); a rejection is counted and
//      the replay says it couldn't open.
//   4. The first-run screen's setup_* commands, through the same bridge (setup/tauriSetup.ts).
//
// Both payloads are the serde JSON of the types in crates/core/src/protocol.rs. Batches are
// parsed again here (protocol/parse.ts) so a mismatched build drops events instead of crashing.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import { parseUiBatch } from "../protocol/parse.ts";
import { type BatchSink, isSessionId, SessionClock, type Transport } from "./transport.ts";

/** The app's commands (crates/app/src/main.rs); the setup ones serve the first-run screen. */
export type TauriCommand =
  | "connect"
  | "command"
  | "session_log"
  | "setup_status"
  | "setup_save"
  | "setup_key"
  | "setup_download"
  | "setup_microphone"
  | "setup_open_microphone_settings"
  | "setup_finish";

/** The two Tauri calls the transports need, injectable so tests don't need a webview. */
export interface TauriBridge {
  invoke(command: TauriCommand, args: Record<string, unknown>): Promise<unknown>;
  /** Creates the channel object passed to `connect`; `onMessage` gets each batch. */
  channel(onMessage: (message: unknown) => void): unknown;
}

export function tauriBridge(): TauriBridge {
  return {
    invoke: (command, args) => invoke(command, args),
    channel: (onMessage) => new Channel<unknown>(onMessage),
  };
}

/** True inside the Tauri webview, where the shell injects its IPC internals. */
export function insideTauri(scope: object): boolean {
  return "__TAURI_INTERNALS__" in scope;
}

export class TauriTransport implements Transport {
  readonly kind = "tauri";
  readonly #bridge: TauriBridge;
  readonly #clock = new SessionClock();
  readonly #pageNow: () => number;
  #sink: BatchSink | null = null;
  malformedBatches = 0;
  skippedEvents = 0;
  /** View rows (a hand-made note, say) skipped while the rest of their view was shown. */
  skippedRows = 0;
  failedCommands = 0;
  failedSessionLogs = 0;
  /** Sessions that started after the first one (idle timeouts): each reset the stage. */
  sessionsStarted = 0;

  constructor(bridge: TauriBridge, pageNow: () => number) {
    this.#bridge = bridge;
    this.#pageNow = pageNow;
  }

  async connect(sink: BatchSink): Promise<void> {
    this.#sink = sink;
    const channel = this.#bridge.channel((message) => this.#receive(message));
    await this.#bridge.invoke("connect", { events: channel });
  }

  send(command: UiCommand): void {
    this.#bridge.invoke("command", { command }).catch(() => {
      this.failedCommands += 1;
    });
  }

  sessionTime(pageNowMs: number): number {
    return this.#clock.now(pageNowMs);
  }

  async sessionLog(id: string): Promise<string | null> {
    if (!isSessionId(id)) {
      this.failedSessionLogs += 1;
      return null;
    }
    try {
      const text = await this.#bridge.invoke("session_log", { id });
      if (typeof text === "string") {
        return text;
      }
    } catch {
      // Unknown id, unreadable or too large: the replay says it couldn't open.
    }
    this.failedSessionLogs += 1;
    return null;
  }

  close(): void {
    this.#sink = null;
  }

  #receive(message: unknown): void {
    const parsed = parseUiBatch(message);
    if (parsed === null) {
      this.malformedBatches += 1;
      return;
    }
    this.skippedEvents += parsed.skipped;
    this.skippedRows += parsed.skippedRows;
    const { at, events } = parsed.batch;
    // A new session restarts session time: what came before it in the batch belongs to the old
    // session, then the stage and the clock start over.
    let begin = 0;
    events.forEach((event, index) => {
      if (event.type !== "SessionStart") {
        return;
      }
      if (index > begin) {
        this.#deliver({ at, events: events.slice(begin, index) });
      }
      this.#clock.reset();
      this.#sink?.reset();
      this.sessionsStarted += 1;
      begin = index;
    });
    this.#deliver(begin === 0 ? parsed.batch : { at, events: events.slice(begin) });
  }

  #deliver(batch: UiBatch): void {
    this.#clock.observe(batch.at, this.#pageNow());
    this.#sink?.batch(batch);
  }
}
