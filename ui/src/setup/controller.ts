// Drives the first-run screen: every action goes to the app through the SetupTransport, and
// every answer goes through the reducer into one signal the components read. Downloads start
// on their own for the files the session needs; a status that says a file is downloading
// without a channel here (after a reload) is asked for again until it isn't.

import { type Signal, signal } from "@preact/signals";
import type { InputDevice } from "../protocol/generated/InputDevice.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { MicChoice } from "../protocol/generated/MicChoice.ts";
import type { SetupEdit } from "../protocol/generated/SetupEdit.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TalkerProvider } from "../protocol/generated/TalkerProvider.ts";
import type { VoiceEngine } from "../protocol/generated/VoiceEngine.ts";
import {
  type Card,
  canStart,
  filesToFetch,
  initialState,
  needsPolling,
  reduce,
  type SetupAction,
  type SetupState,
} from "./state.ts";
import { reason, type SetupTransport } from "./transport.ts";

/** How often to ask again while a file downloads with no channel to this screen. */
export const SETUP_POLL_MS = 2000;

export type Schedule = (delayMs: number, run: () => void) => void;

const defaultSchedule: Schedule = (delayMs, run) => {
  setTimeout(run, delayMs);
};

export class SetupController {
  readonly state: Signal<SetupState>;
  readonly #transport: SetupTransport;
  readonly #schedule: Schedule;
  /** Status requests issued and the newest one applied, so an old answer never wins. */
  #issued = 0;
  #applied = 0;
  #polling = false;

  constructor(
    transport: SetupTransport,
    status: SetupStatus | null,
    schedule: Schedule = defaultSchedule,
  ) {
    this.#transport = transport;
    this.#schedule = schedule;
    this.state = signal(initialState(status));
  }

  get value(): SetupState {
    return this.state.value;
  }

  /** Reads the status (when none was given) and starts the downloads the session needs. */
  async load(): Promise<void> {
    if (this.state.value.status === null) {
      await this.refresh();
    } else {
      this.#settled();
    }
  }

  /** Asks the app again (window focus, a finished download, polling). Quiet on failure. */
  async refresh(): Promise<void> {
    const ticket = ++this.#issued;
    try {
      const status = await this.#transport.status();
      if (ticket > this.#applied) {
        this.#applied = ticket;
        this.#dispatch({ type: "loaded", status });
        this.#settled();
      }
    } catch (error) {
      if (this.state.value.status === null) {
        this.#dispatch({ type: "loadFailed", reason: reason(error) });
      }
    }
  }

  pickTalker(provider: TalkerProvider): Promise<void> {
    if (this.state.value.status?.talker.provider === provider) {
      return Promise.resolve();
    }
    return this.#save("thinking", { kind: "Talker", provider, model: null });
  }

  pickOllamaModel(model: string): Promise<void> {
    return this.#save("thinking", { kind: "Talker", provider: "Ollama", model });
  }

  /**
   * Saves a pasted key. The caller has already cleared its field; the value lives only in this
   * call. An ElevenLabs key also makes ElevenLabs the voice.
   */
  saveKey(name: KeyName, value: string): Promise<void> {
    const card: Card = name === "ElevenLabs" ? "voice" : "thinking";
    return this.#request(card, async () => {
      const saved = await this.#transport.saveKey(name, value);
      if (name !== "ElevenLabs") {
        return saved;
      }
      return this.#transport.save({ kind: "Voice", engine: "ElevenLabs" });
    });
  }

  useVoice(engine: VoiceEngine): Promise<void> {
    return this.#save("voice", { kind: "Voice", engine });
  }

  openVoiceKey(open: boolean): void {
    this.#dispatch({ type: "voiceKey", open });
  }

  allowMicrophone(): Promise<void> {
    return this.#request("microphone", () => this.#transport.requestMicrophone());
  }

  /** "Open Microphone settings": a failure is said on the microphone card. */
  async openMicrophoneSettings(): Promise<void> {
    try {
      await this.#transport.openMicrophoneSettings();
    } catch (error) {
      this.#dispatch({ type: "refused", card: "microphone", reason: reason(error) });
    }
  }

  pickMode(mode: MicChoice): Promise<void> {
    if (this.state.value.status?.mic_mode === mode) {
      return Promise.resolve();
    }
    return this.#save("microphone", { kind: "Mic", mode });
  }

  pickInput(device: InputDevice): Promise<void> {
    return this.#save("microphone", { kind: "Input", device });
  }

  retryDownloads(): void {
    this.#dispatch({ type: "retry" });
    this.#fetchMissing();
  }

  /** "Start talking": the app checks again and starts the session. */
  async start(): Promise<void> {
    if (!canStart(this.state.value)) {
      return;
    }
    await this.#finish();
  }

  /**
   * Esc or ⌘, on the settings screen: the app closes it at once when nothing was saved, and
   * otherwise checks again and restarts the session (or says what is missing).
   */
  async leave(): Promise<void> {
    const state = this.state.value;
    if (state.status === null || state.finished || state.busy.size > 0) {
      return;
    }
    await this.#finish();
  }

  async #finish(): Promise<void> {
    this.#dispatch({ type: "busy", card: "start" });
    try {
      await this.#transport.finish();
      this.#dispatch({ type: "finished" });
    } catch (error) {
      this.#dispatch({ type: "refused", card: "start", reason: reason(error) });
      await this.refresh();
    }
  }

  #save(card: Card, edit: SetupEdit): Promise<void> {
    return this.#request(card, () => this.#transport.save(edit));
  }

  async #request(card: Card, run: () => Promise<SetupStatus>): Promise<void> {
    this.#dispatch({ type: "busy", card });
    const ticket = ++this.#issued;
    try {
      const status = await run();
      const fresh = ticket > this.#applied;
      if (fresh) {
        this.#applied = ticket;
      }
      this.#dispatch({ type: "answered", card, status: fresh ? status : null });
      this.#settled();
    } catch (error) {
      this.#dispatch({ type: "refused", card, reason: reason(error) });
    }
  }

  #dispatch(action: SetupAction): void {
    this.state.value = reduce(this.state.value, action);
  }

  /** After a new status: start what needs downloading, and poll what downloads elsewhere. */
  #settled(): void {
    this.#fetchMissing();
    if (needsPolling(this.state.value) && !this.#polling) {
      this.#polling = true;
      this.#schedule(SETUP_POLL_MS, () => {
        this.#polling = false;
        void this.refresh();
      });
    }
  }

  #fetchMissing(): void {
    for (const model of filesToFetch(this.state.value)) {
      this.#fetch(model);
    }
  }

  #fetch(model: string): void {
    this.#dispatch({ type: "transferStarted", model });
    let ended = false;
    this.#transport
      .download(model, (event) => {
        ended ||= event.kind !== "Progress";
        this.#dispatch({ type: "transfer", model, event });
      })
      .then(
        () => {
          if (!ended) {
            // Already downloading for an earlier screen: the status follows it (polling).
            this.#dispatch({ type: "transferLost", model });
          }
          return this.refresh();
        },
        (error: unknown) => {
          if (!ended) {
            this.#dispatch({ type: "transferRefused", model, reason: reason(error) });
          }
          return this.refresh();
        },
      );
  }
}
