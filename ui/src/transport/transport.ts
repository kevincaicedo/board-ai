// The only way the UI talks to the rest of BoardAI (LLD §3.7, D-015). Components never see Tauri;
// they see a Transport, whose implementations are Tauri (live), replay (a file) and a fake (tests).

import type { ChooseWhat } from "../protocol/generated/ChooseWhat.ts";
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
  /**
   * A past session's `events.jsonl`, by id, for a replay; `null` when it can't be had (an
   * unknown id, a log too large to replay, no backend). Never throws.
   */
  sessionLog(id: string): Promise<string | null>;
  /**
   * The Sources tray's "choose a folder" / "a file" (D-184): the app shows the system's chooser
   * itself and shares what it picks, so no path ever comes from the UI. Resolves with a sentence
   * to show when it couldn't, else `null` (a cancel included). Never throws.
   */
  chooseSource(what: ChooseWhat, keep: boolean): Promise<string | null>;
  close(): void;
}

/**
 * Session ids are `<date>-<time>` with an optional `-n` (the store's `SessionId`). Anything else
 * is refused before it reaches the shell or a URL, so an id can never name a path.
 */
const SESSION_ID = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$/;

export function isSessionId(id: string): boolean {
  return SESSION_ID.test(id) && !id.includes("..");
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

  /** A new session began: its stamps restart near zero, so the next batch sets the clock. */
  reset(): void {
    this.#offsetMs = null;
  }

  now(pageNowMs: number): number {
    if (this.#offsetMs === null) {
      return 0;
    }
    return Math.max(0, pageNowMs - this.#offsetMs);
  }
}
