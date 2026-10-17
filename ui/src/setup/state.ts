// The first-run screen's and the settings page's state (LLD §3.8): the app's last status,
// requests in flight per card (a settings section is a card too), refusals in plain words, and
// the downloads this screen started. A pure reducer, so every transition is tested with
// node --test; the controller feeds it.
//
// Whether the session may start is the app's decision (core::setup::blockers, sent in the
// status as `blockers`); the screen adds only that nothing is in flight.

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { ModelFileStatus } from "../protocol/generated/ModelFileStatus.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { downloadProblem } from "./words.ts";

/** The first run's three cards and its start button, plus the settings page's other sections. */
export type Card = "thinking" | "voice" | "microphone" | "start" | "keys" | "coding" | "memory";

/** A download this screen started, as its channel reports it. */
export interface Transfer {
  readonly receivedBytes: number;
  readonly totalBytes: number;
  /** The channel is open: more progress will come. */
  readonly live: boolean;
  /** Why it stopped, in plain words. */
  readonly problem: string | null;
}

export interface SetupState {
  readonly status: SetupStatus | null;
  /** The status couldn't be read at all. */
  readonly loadProblem: string | null;
  /** Cards with a request in flight; their controls wait. */
  readonly busy: ReadonlySet<Card>;
  /** The last refusal per card; the card's next answer clears it. */
  readonly problems: Readonly<Record<Card, string | null>>;
  readonly transfers: ReadonlyMap<string, Transfer>;
  /** "Add a key" for ElevenLabs is open. */
  readonly addingVoiceKey: boolean;
  /** What the last "Rebuild the index" did, this time the page is open. */
  readonly rebuilt: IndexRebuilt | null;
  /** The session started: the stage takes over. */
  readonly finished: boolean;
}

export type SetupAction =
  | { readonly type: "loaded"; readonly status: SetupStatus }
  | { readonly type: "loadFailed"; readonly reason: string }
  | { readonly type: "busy"; readonly card: Card }
  /** `status` is null when a newer status already arrived (it is kept). */
  | { readonly type: "answered"; readonly card: Card; readonly status: SetupStatus | null }
  | { readonly type: "refused"; readonly card: Card; readonly reason: string }
  | { readonly type: "transferStarted"; readonly model: string }
  | { readonly type: "transfer"; readonly model: string; readonly event: DownloadEvent }
  /** The app refused the download before it began (its words). */
  | { readonly type: "transferRefused"; readonly model: string; readonly reason: string }
  /** The call ended without a verdict: the file was already downloading (after a reload). */
  | { readonly type: "transferLost"; readonly model: string }
  /** "Try again" on the voice card: failed downloads may start again. */
  | { readonly type: "retry" }
  | { readonly type: "voiceKey"; readonly open: boolean }
  | { readonly type: "rebuilt"; readonly rebuilt: IndexRebuilt }
  | { readonly type: "finished" };

const NO_PROBLEMS: Readonly<Record<Card, string | null>> = {
  thinking: null,
  voice: null,
  microphone: null,
  start: null,
  keys: null,
  coding: null,
  memory: null,
};

export function initialState(status: SetupStatus | null): SetupState {
  return {
    status,
    loadProblem: null,
    busy: new Set(),
    problems: NO_PROBLEMS,
    transfers: new Map(),
    addingVoiceKey: false,
    rebuilt: null,
    finished: false,
  };
}

function without(busy: ReadonlySet<Card>, card: Card): ReadonlySet<Card> {
  const next = new Set(busy);
  next.delete(card);
  return next;
}

function fileOf(status: SetupStatus | null, model: string): ModelFileStatus | undefined {
  if (status === null) {
    return undefined;
  }
  return [...status.hearing_files, ...status.voice_files].find((file) => file.id === model);
}

function withTransfer(state: SetupState, model: string, transfer: Transfer): SetupState {
  const transfers = new Map(state.transfers);
  transfers.set(model, transfer);
  return { ...state, transfers };
}

function transferred(state: SetupState, model: string, event: DownloadEvent): SetupState {
  const before = state.transfers.get(model);
  const file = fileOf(state.status, model);
  const totalBytes = before?.totalBytes ?? file?.size_bytes ?? 0;
  const receivedBytes = before?.receivedBytes ?? 0;
  switch (event.kind) {
    case "Progress":
      return withTransfer(state, model, {
        receivedBytes: event.received_bytes,
        totalBytes: event.total_bytes,
        live: true,
        problem: null,
      });
    case "Done":
      return withTransfer(state, model, {
        receivedBytes: totalBytes,
        totalBytes,
        live: false,
        problem: null,
      });
    case "Failed":
      return withTransfer(state, model, {
        receivedBytes,
        totalBytes,
        live: false,
        problem: downloadProblem(event.problem),
      });
  }
}

export function reduce(state: SetupState, action: SetupAction): SetupState {
  switch (action.type) {
    case "loaded":
      return { ...state, status: action.status, loadProblem: null };
    case "loadFailed":
      return { ...state, loadProblem: action.reason };
    case "busy": {
      const busy = new Set(state.busy);
      busy.add(action.card);
      return { ...state, busy, problems: { ...state.problems, [action.card]: null } };
    }
    case "answered":
      return {
        ...state,
        status: action.status ?? state.status,
        loadProblem: action.status === null ? state.loadProblem : null,
        busy: without(state.busy, action.card),
        problems: { ...state.problems, [action.card]: null },
        addingVoiceKey: action.card === "voice" ? false : state.addingVoiceKey,
      };
    case "refused":
      return {
        ...state,
        busy: without(state.busy, action.card),
        problems: { ...state.problems, [action.card]: action.reason },
      };
    case "transferStarted": {
      const file = fileOf(state.status, action.model);
      return withTransfer(state, action.model, {
        receivedBytes: file?.have_bytes ?? 0,
        totalBytes: file?.size_bytes ?? 0,
        live: true,
        problem: null,
      });
    }
    case "transfer":
      return transferred(state, action.model, action.event);
    case "transferRefused": {
      const before = state.transfers.get(action.model);
      return withTransfer(state, action.model, {
        receivedBytes: before?.receivedBytes ?? 0,
        totalBytes: before?.totalBytes ?? fileOf(state.status, action.model)?.size_bytes ?? 0,
        live: false,
        problem: action.reason,
      });
    }
    case "transferLost": {
      const before = state.transfers.get(action.model);
      if (before === undefined || before.problem !== null) {
        return state;
      }
      return withTransfer(state, action.model, { ...before, live: false });
    }
    case "retry": {
      const transfers = new Map(
        [...state.transfers].filter(([, transfer]) => transfer.problem === null),
      );
      return { ...state, transfers, problems: { ...state.problems, voice: null } };
    }
    case "voiceKey":
      return { ...state, addingVoiceKey: action.open };
    case "rebuilt":
      return {
        ...state,
        rebuilt: action.rebuilt,
        busy: without(state.busy, "memory"),
        problems: { ...state.problems, memory: null },
      };
    case "finished":
      return { ...state, finished: true, busy: without(state.busy, "start") };
  }
}

/**
 * A change is on its way to the app: leaving waits for it, so the restart sees it. Opening the
 * vault and rebuilding the index change no setting, so they never hold the page open.
 */
export function savesInFlight(state: SetupState): boolean {
  return [...state.busy].some((card) => card !== "memory");
}

/** "Start talking" is enabled: the app names no blocker and nothing is in flight. */
export function canStart(state: SetupState): boolean {
  return (
    state.status !== null &&
    !state.finished &&
    state.busy.size === 0 &&
    state.status.blockers.length === 0
  );
}

export interface GroupProgress {
  readonly receivedBytes: number;
  readonly totalBytes: number;
  /** Every file is verified. */
  readonly ready: boolean;
  /** A file is downloading now. */
  readonly running: boolean;
  /** Why a file stopped, in plain words. */
  readonly problem: string | null;
}

/** One row of the voice card (Kokoro's files, or Whisper and the voice detector). */
export function groupProgress(
  files: readonly ModelFileStatus[],
  transfers: ReadonlyMap<string, Transfer>,
): GroupProgress {
  let receivedBytes = 0;
  let totalBytes = 0;
  let running = false;
  let problem: string | null = null;
  for (const file of files) {
    totalBytes += file.size_bytes;
    const transfer = transfers.get(file.id);
    if (file.state === "Ready") {
      receivedBytes += file.size_bytes;
      continue;
    }
    const have = transfer === undefined ? file.have_bytes : transfer.receivedBytes;
    receivedBytes += Math.min(have, file.size_bytes);
    running ||= file.state === "Downloading" || transfer?.live === true;
    problem ??= transfer?.problem ?? null;
  }
  const ready = files.length > 0 && files.every((file) => file.state === "Ready");
  return { receivedBytes, totalBytes, ready, running: running && !ready, problem };
}

/**
 * Files to download now: the hearing files always, Kokoro's while it is the voice. A file
 * already downloading, or one that failed (until "Try again"), is left alone.
 */
export function filesToFetch(state: SetupState): string[] {
  const status = state.status;
  if (status === null || state.finished) {
    return [];
  }
  const wanted = [
    ...status.hearing_files,
    ...(status.voice === "Kokoro" ? status.voice_files : []),
  ];
  return wanted
    .filter((file) => file.state === "Missing" || file.state === "Partial")
    .filter((file) => {
      const transfer = state.transfers.get(file.id);
      return transfer === undefined || (!transfer.live && transfer.problem === null);
    })
    .map((file) => file.id);
}

/** A file is downloading without a channel to this screen (a reload): ask the app again. */
export function needsPolling(state: SetupState): boolean {
  const status = state.status;
  if (status === null || state.finished) {
    return false;
  }
  return [...status.hearing_files, ...status.voice_files].some(
    (file) => file.state === "Downloading" && state.transfers.get(file.id)?.live !== true,
  );
}
