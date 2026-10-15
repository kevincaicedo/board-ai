// The only way the UI talks to the rest of BoardAI (LLD §3.7, D-015). Components never see Tauri;
// they see a Transport, whose implementations are Tauri (live), replay (a file) and a fake (tests).

import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiCommand } from "../protocol/generated/UiCommand.ts";

export interface BatchSink {
  /** Events in order, stamped with the session time of the core step that produced them. */
  batch(batch: UiBatch): void;
  /** Forget all session state: a replay seeked backwards and re-delivers from the start. */
  reset(): void;
}

export interface Transport {
  readonly kind: "tauri" | "replay" | "fake";
  /** Starts delivery to `sink`. Called once. */
  connect(sink: BatchSink): Promise<void>;
  send(command: UiCommand): void;
  /**
   * The session time to render at, for the page's monotonic time (performance.now). A
   * pull-based transport (replay) delivers every batch that became due during this call, so a
   * frame always renders the events it should.
   */
  sessionTime(pageNowMs: number): number;
  close(): void;
}

/** Live mapping from page time to session time, following the stamps on each batch (LLD §4). */
export class SessionClock {
  #offsetMs: number | null = null;

  /** A batch stamped `at` arrived at page time `pageNowMs`. */
  observe(at: number, pageNowMs: number): void {
    if (this.#offsetMs === null || at > this.now(pageNowMs)) {
      // The UI's clock was behind the core's: jump forward so new strokes start where the core
      // scheduled them. It never steps back, so animations never rewind; a late batch simply
      // starts its strokes slightly in the past and they catch up.
      this.#offsetMs = pageNowMs - at;
    }
  }

  now(pageNowMs: number): number {
    if (this.#offsetMs === null) {
      return 0;
    }
    return Math.max(0, pageNowMs - this.#offsetMs);
  }
}
