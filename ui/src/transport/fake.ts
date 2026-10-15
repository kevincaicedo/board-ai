// An in-memory transport for tests: push batches in, read commands out, set the clock by hand.

import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import type { BatchSink, Transport } from "./transport.ts";

export class FakeTransport implements Transport {
  readonly kind = "fake";
  readonly sent: UiCommand[] = [];
  /** The session time the next frame renders at. */
  time = 0;
  #sink: BatchSink | null = null;

  async connect(sink: BatchSink): Promise<void> {
    this.#sink = sink;
  }

  get connected(): boolean {
    return this.#sink !== null;
  }

  push(batch: UiBatch): void {
    if (this.#sink === null) {
      throw new Error("FakeTransport.push before connect");
    }
    this.time = Math.max(this.time, batch.at);
    this.#sink.batch(batch);
  }

  reset(): void {
    this.#sink?.reset();
  }

  send(command: UiCommand): void {
    this.sent.push(command);
  }

  sessionTime(_pageNowMs: number): number {
    return this.time;
  }

  close(): void {
    this.#sink = null;
  }
}
