// Plays a session's events.jsonl with no backend (LLD §4 replay mode): play, pause, seek and
// speed, on the session's own timing. Delivery is pulled by the frame loop through sessionTime,
// so a paused or seeked replay renders exactly the frame a live session showed at that time.

import type { UiCommand } from "../protocol/generated/UiCommand.ts";
import type { Chapter, ReplayLog } from "./log.ts";
import type { BatchSink, Transport } from "./transport.ts";

export const REPLAY_SPEEDS = [0.5, 1, 1.5, 2] as const;
export type ReplaySpeed = (typeof REPLAY_SPEEDS)[number];

export interface ReplayState {
  readonly playing: boolean;
  readonly durationMs: number;
  readonly speed: ReplaySpeed;
  readonly chapters: readonly Chapter[];
}

export class ReplayTransport implements Transport {
  readonly kind = "replay";
  readonly log: ReplayLog;
  #sink: BatchSink | null = null;
  #nextBatch = 0;
  #playing = false;
  #speed: ReplaySpeed = 1;
  /** Playhead at page time #anchorPageMs. */
  #anchorPositionMs = 0;
  #anchorPageMs = 0;
  #listeners = new Set<() => void>();
  /** Commands have nowhere to go in a replay; they are counted for the dev overlay. */
  ignoredCommands = 0;

  constructor(log: ReplayLog) {
    this.log = log;
  }

  async connect(sink: BatchSink): Promise<void> {
    this.#sink = sink;
  }

  send(_command: UiCommand): void {
    this.ignoredCommands += 1;
  }

  close(): void {
    this.#sink = null;
    this.#listeners.clear();
  }

  get state(): ReplayState {
    return {
      playing: this.#playing,
      durationMs: this.log.durationMs,
      speed: this.#speed,
      chapters: this.log.chapters,
    };
  }

  /** Called when play, pause, speed or the end change (not every frame). */
  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  position(pageNowMs: number): number {
    if (!this.#playing) {
      return this.#anchorPositionMs;
    }
    const position = this.#anchorPositionMs + (pageNowMs - this.#anchorPageMs) * this.#speed;
    return Math.min(position, this.log.durationMs);
  }

  sessionTime(pageNowMs: number): number {
    const position = this.position(pageNowMs);
    if (this.#playing && position >= this.log.durationMs) {
      this.#anchorPositionMs = this.log.durationMs;
      this.#anchorPageMs = pageNowMs;
      this.#playing = false;
      this.#notify();
    }
    this.#deliverThrough(position);
    return position;
  }

  play(pageNowMs: number): void {
    if (this.#playing) {
      return;
    }
    const restart = this.#anchorPositionMs >= this.log.durationMs;
    if (restart) {
      this.seek(0, pageNowMs);
    }
    this.#anchorPageMs = pageNowMs;
    this.#playing = true;
    this.#notify();
  }

  pause(pageNowMs: number): void {
    if (!this.#playing) {
      return;
    }
    this.#anchorPositionMs = this.position(pageNowMs);
    this.#anchorPageMs = pageNowMs;
    this.#playing = false;
    this.#notify();
  }

  toggle(pageNowMs: number): void {
    if (this.#playing) {
      this.pause(pageNowMs);
    } else {
      this.play(pageNowMs);
    }
  }

  seek(positionMs: number, pageNowMs: number): void {
    const target = Math.max(0, Math.min(positionMs, this.log.durationMs));
    const delivered = this.log.batches[this.#nextBatch - 1];
    if (delivered !== undefined && delivered.at > target) {
      this.#nextBatch = 0;
      this.#sink?.reset();
    }
    this.#anchorPositionMs = target;
    this.#anchorPageMs = pageNowMs;
    this.#deliverThrough(target);
    this.#notify();
  }

  setSpeed(speed: ReplaySpeed, pageNowMs: number): void {
    this.#anchorPositionMs = this.position(pageNowMs);
    this.#anchorPageMs = pageNowMs;
    this.#speed = speed;
    this.#notify();
  }

  #deliverThrough(position: number): void {
    const sink = this.#sink;
    if (sink === null) {
      return;
    }
    const batches = this.log.batches;
    while (this.#nextBatch < batches.length) {
      const batch = batches[this.#nextBatch];
      if (batch === undefined || batch.at > position) {
        break;
      }
      this.#nextBatch += 1;
      sink.batch(batch);
    }
  }

  #notify(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }
}
