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
//
// Both payloads are the serde JSON of the types in crates/core/src/protocol.rs. Batches are
// parsed again here (protocol/parse.ts) so a mismatched build drops events instead of crashing.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import { parseUiBatch } from "../protocol/parse.ts";
import { type BatchSink, SessionClock, type Transport } from "./transport.ts";

/** The two Tauri calls the transport needs, injectable so tests don't need a webview. */
export interface TauriBridge {
  invoke(command: "connect" | "command", args: Record<string, unknown>): Promise<unknown>;
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
  failedCommands = 0;

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
    this.#clock.observe(parsed.batch.at, this.#pageNow());
    this.#sink?.batch(parsed.batch);
  }
}
