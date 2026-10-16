// The words the views put under titles: days relative to the day the view was read ("today",
// "Oct 12"), session lengths ("18 min"), a replay clock ("14:32") and the library's spoken hint.
// Pure, so a replayed view reads exactly as it did live (the day comes with the view).

import type { DigestCard } from "../protocol/generated/DigestCard.ts";
import type { NoteRow } from "../protocol/generated/NoteRow.ts";
import type { SessionCard } from "../protocol/generated/SessionCard.ts";
import type { TaskRow } from "../protocol/generated/TaskRow.ts";

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** "today", "Oct 12", or "Oct 12, 2025" in another year; empty for an unknown day. */
export function dayLabel(date: string, today: string): string {
  const match = DAY.exec(date);
  if (match === null) {
    return "";
  }
  if (date === today) {
    return "today";
  }
  const [, year, month, day] = match;
  const name = MONTHS[Number(month) - 1];
  if (name === undefined) {
    return "";
  }
  const label = `${name} ${Number(day)}`;
  return today.startsWith(`${year}-`) ? label : `${label}, ${year}`;
}

/** The first letter capitalized, for a label that starts a line ("Today · Request path"). */
export function capitalized(text: string): string {
  return text === "" ? text : `${text.charAt(0).toUpperCase()}${text.slice(1)}`;
}

/** "18 min", "1 h 5 min"; under half a minute is "1 min"; empty when unknown (zero). */
export function durationLabel(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) {
    return "";
  }
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) {
    return `${minutes} min`;
  }
  const rest = minutes % 60;
  return rest === 0 ? `${Math.floor(minutes / 60)} h` : `${Math.floor(minutes / 60)} h ${rest} min`;
}

/** A replay position, "14:32", or "1:02:05" past an hour. */
export function clockLabel(ms: number): string {
  const seconds = Math.max(0, Math.floor((Number.isFinite(ms) ? ms : 0) / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = String(seconds % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
}

/** Parts joined with the canvas's middle dot, empty parts left out. */
export function joined(...parts: readonly string[]): string {
  return parts.filter((part) => part !== "").join(" · ");
}

/** "Oct 14 · 18 min" under a library card. */
export function sessionMeta(card: SessionCard, today: string): string {
  return joined(capitalized(dayLabel(card.date, today)), durationLabel(card.duration_ms));
}

/** What a replay keeps (PRODUCT §7.6, law 10): the captions, never your voice. */
export const REPLAY_NOTE = "your voice isn't kept, only captions";

/** The line under a replay's title (7b): "Replay · Oct 12 · your voice isn't kept, only captions". */
export function replaySubtitle(card: SessionCard, today: string): string {
  return joined("Replay", dayLabel(card.date, today), REPLAY_NOTE);
}

/** Titles are "Topic · detail"; the topic, at most three words, is how you'd name it aloud. */
export function shortTitle(title: string): string {
  const topic = title.split(/\s[·:—-]\s|:\s/)[0] ?? title;
  return topic.trim().split(/\s+/).slice(0, 3).join(" ");
}

/** The canvas's line under "Sessions", naming a session you could ask for. */
export const LIBRARY_HINT_FALLBACK = "Say “play the Raft one,” or pick a card.";

export function libraryHint(sessions: readonly SessionCard[]): string {
  const named = sessions.find((session) => shortTitle(session.title) !== "");
  return named === undefined
    ? LIBRARY_HINT_FALLBACK
    : `Say “play the ${shortTitle(named.title)} one,” or pick a card.`;
}

/** "Request path · today": where a task came from, or its project. */
export function taskMeta(task: TaskRow, today: string): string {
  if (task.from === null) {
    return task.project;
  }
  return joined(shortTitle(task.from.title), dayLabel(task.from.date, today)) || task.project;
}

/** "Today · Request path": when and where a note was taken. */
export function noteMeta(note: NoteRow, today: string): string {
  return joined(
    capitalized(dayLabel(note.date, today)),
    note.from === null ? "" : shortTitle(note.from.title),
  );
}

function counted(count: number, one: string, many: string): string {
  if (count <= 0) {
    return "";
  }
  return count === 1 ? `1 ${one}` : `${count} ${many}`;
}

/** "1 diagram · 2 notes · 1 task · 1 open question" under the latest digest; zeros left out. */
export function digestCounts(card: DigestCard): string {
  return joined(
    counted(card.session.cover.length > 0 ? 1 : 0, "diagram", "diagrams"),
    counted(card.notes, "note", "notes"),
    counted(card.tasks, "task", "tasks"),
    counted(card.open_questions, "open question", "open questions"),
  );
}
