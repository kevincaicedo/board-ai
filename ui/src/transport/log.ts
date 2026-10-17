// Parses a session's `events.jsonl` into batches the replay transport plays back (LLD §3.6, §4).
// Bad lines never stop a replay: each one is skipped and counted by reason.

import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { parseLogLine } from "../protocol/parse.ts";

/** Lines read from one file; past this the rest is skipped and counted as `overLimit`. */
export const REPLAY_LINES_MAX = 200_000;
/** Bytes in one line; a longer line is skipped, so one corrupt line can't stall parsing. */
export const REPLAY_LINE_BYTES_MAX = 1_048_576;
/** Session time the replay keeps running after the last event, so end animations finish. */
export const REPLAY_TAIL_MS = 2_000;

export interface SkipCounts {
  /** Not JSON, wrong version, bad time, or an unknown or malformed event. */
  malformed: number;
  /** Well-formed lines meant for re-simulation (`"k":"input"`), not the UI. */
  otherKind: number;
  overLimit: number;
  /** View rows skipped inside events that were kept (a hand-made note a view couldn't read). */
  rows: number;
}

export type ChapterKind =
  | "Idle"
  | "Listening"
  | "Explaining"
  | "Detour"
  | "Code"
  | "Compare"
  | "Recap"
  | "Capture"
  | "Resume"
  | "Drawing";

export interface Chapter {
  readonly at: number;
  readonly kind: ChapterKind;
}

export interface ReplayLog {
  /** Events grouped by time, in time order; events at the same time keep file order. */
  readonly batches: readonly UiBatch[];
  readonly durationMs: number;
  readonly chapters: readonly Chapter[];
  readonly skipped: SkipCounts;
}

export function parseReplayLog(text: string): ReplayLog {
  const skipped: SkipCounts = { malformed: 0, otherKind: 0, overLimit: 0, rows: 0 };
  const lines: { t: number; event: UiEvent; order: number }[] = [];
  let lineCount = 0;
  let begin = 0;
  while (begin <= text.length) {
    const newline = text.indexOf("\n", begin);
    const end = newline === -1 ? text.length : newline;
    const raw = text.slice(begin, end).trim();
    begin = end + 1;
    if (raw === "") {
      if (newline === -1) {
        break;
      }
      continue;
    }
    lineCount += 1;
    if (lineCount > REPLAY_LINES_MAX || raw.length > REPLAY_LINE_BYTES_MAX) {
      skipped.overLimit += 1;
    } else {
      const result = parseLogLine(raw);
      switch (result.kind) {
        case "ui":
          lines.push({ t: result.line.t, event: result.line.event, order: lines.length });
          skipped.rows += result.skippedRows;
          break;
        case "other":
          skipped.otherKind += 1;
          break;
        case "malformed":
          skipped.malformed += 1;
          break;
      }
    }
    if (newline === -1) {
      break;
    }
  }
  // The log is append-only and monotonic, but a hand-edited file may not be: order by time and
  // keep file order for ties.
  lines.sort((a, b) => a.t - b.t || a.order - b.order);
  const batches: UiBatch[] = [];
  for (const line of lines) {
    const last = batches[batches.length - 1];
    if (last !== undefined && last.at === line.t) {
      last.events.push(line.event);
    } else {
      batches.push({ at: line.t, events: [line.event] });
    }
  }
  const lastAt = batches[batches.length - 1]?.at ?? 0;
  return { batches, durationMs: lastAt + REPLAY_TAIL_MS, chapters: chaptersOf(batches), skipped };
}

/**
 * Scrubber chapters (PRODUCT §7.6): where the session changes what it is doing. A capture
 * chapter starts with the utterance that asked for it, not when the chip lands.
 */
export function chaptersOf(batches: readonly UiBatch[]): Chapter[] {
  const chapters: Chapter[] = [{ at: 0, kind: "Idle" }];
  const add = (at: number, kind: ChapterKind): void => {
    const last = chapters[chapters.length - 1];
    if (last === undefined || last.kind !== kind) {
      chapters.push({ at: Math.max(at, last?.at ?? 0), kind });
    }
  };
  let listened = false;
  let explained = false;
  let userSpeechAt: number | null = null;
  let userSpeaking = false;
  for (const batch of batches) {
    for (const event of batch.events) {
      switch (event.type) {
        case "Orb":
          if (event.state === "Listening" && !listened) {
            listened = true;
            add(batch.at, "Listening");
          }
          break;
        case "Caption":
          if (event.who === "User" && !userSpeaking) {
            userSpeechAt = batch.at;
          }
          userSpeaking = event.who === "User" && !event.is_final;
          break;
        case "OpStart":
          if (!explained) {
            explained = true;
            add(batch.at, "Explaining");
          }
          break;
        case "LayerPush":
          add(batch.at, "Detour");
          break;
        case "LayerPop":
          add(batch.at, "Resume");
          break;
        case "SurfaceShow":
          add(batch.at, event.surface.kind);
          break;
        case "Chip":
          if (event.kind !== "Source") {
            add(
              userSpeechAt !== null && userSpeechAt <= batch.at ? userSpeechAt : batch.at,
              "Capture",
            );
          }
          break;
        case "DrawMode":
          if (event.drawing) {
            add(batch.at, "Drawing");
          }
          break;
        case "Acknowledge":
        case "Thread":
        case "Freeze":
        case "Resume":
        case "Camera":
        case "SurfaceUpdate":
        case "SurfaceDismiss":
        case "Problem":
        case "SessionStart":
        case "ViewShow":
        case "ViewDismiss":
        case "OpenSettings":
        case "SessionEnd":
        case "UserShape":
        case "UserShapeRemove":
        case "LabelHint":
          break;
      }
    }
  }
  return chapters;
}
