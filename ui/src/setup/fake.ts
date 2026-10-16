// An in-memory SetupTransport for tests and the visual check (`?setup=<preset>` outside Tauri).
// It answers with statuses a test or preset gives it; it doesn't decide blockers itself (the
// app does), so a test states the status it expects the app to send.

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { ModelFileStatus } from "../protocol/generated/ModelFileStatus.ts";
import type { SetupEdit } from "../protocol/generated/SetupEdit.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { SetupTransport } from "./transport.ts";

export type FakeCall =
  | { readonly kind: "status" }
  | { readonly kind: "save"; readonly edit: SetupEdit }
  | { readonly kind: "saveKey"; readonly name: KeyName; readonly length: number }
  | { readonly kind: "download"; readonly model: string }
  | { readonly kind: "requestMicrophone" }
  | { readonly kind: "finish" };

interface PendingDownload {
  readonly emit: (event: DownloadEvent) => void;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
}

export class FakeSetupTransport implements SetupTransport {
  /** What every call answers with unless an `on…` hook says otherwise. */
  current: SetupStatus;
  readonly calls: FakeCall[] = [];
  readonly downloads = new Map<string, PendingDownload>();
  onSave: (edit: SetupEdit) => SetupStatus = () => this.current;
  onKey: (name: KeyName) => SetupStatus = () => this.current;
  onMicrophone: () => SetupStatus = () => this.current;
  /** A reason makes the next call of that kind reject with it. */
  refuse: Partial<Record<FakeCall["kind"], string>> = {};
  /** Events each new download emits at once (the visual check freezes a bar this way). */
  initialEvents = new Map<string, DownloadEvent>();

  constructor(status: SetupStatus) {
    this.current = status;
  }

  async status(): Promise<SetupStatus> {
    this.#record({ kind: "status" });
    return this.current;
  }

  async save(edit: SetupEdit): Promise<SetupStatus> {
    this.#record({ kind: "save", edit });
    this.current = this.onSave(edit);
    return this.current;
  }

  async saveKey(name: KeyName, value: string): Promise<SetupStatus> {
    // Only the length is kept: a test can check a key went out without holding it.
    this.#record({ kind: "saveKey", name, length: value.length });
    this.current = this.onKey(name);
    return this.current;
  }

  download(model: string, onEvent: (event: DownloadEvent) => void): Promise<void> {
    try {
      this.#record({ kind: "download", model });
    } catch (error) {
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      this.downloads.set(model, { emit: onEvent, resolve, reject });
      const first = this.initialEvents.get(model);
      if (first !== undefined) {
        onEvent(first);
      }
    });
  }

  async requestMicrophone(): Promise<SetupStatus> {
    this.#record({ kind: "requestMicrophone" });
    this.current = this.onMicrophone();
    return this.current;
  }

  async finish(): Promise<void> {
    this.#record({ kind: "finish" });
  }

  #record(call: FakeCall): void {
    this.calls.push(call);
    const refusal = this.refuse[call.kind];
    if (refusal !== undefined) {
      delete this.refuse[call.kind];
      throw refusal;
    }
  }
}

const MIB = 1024 * 1024;

function file(id: string, sizeBytes: number, state: ModelFileStatus["state"]): ModelFileStatus {
  return {
    id,
    state,
    have_bytes: state === "Ready" ? sizeBytes : 0,
    size_bytes: sizeBytes,
  };
}

/** A first launch: nothing saved, nothing downloaded, the microphone not asked. */
export function firstLaunch(): SetupStatus {
  return {
    complete: false,
    settings_problem: null,
    talker: {
      provider: "Claude",
      model: "claude-opus-5-5",
      key: "Missing",
      ollama: { state: "Unreachable" },
    },
    keys: [],
    voice: "Kokoro",
    voice_files: [
      file("kokoro", 310 * MIB, "Missing"),
      file("kokoro-voice-af-heart", MIB / 2, "Missing"),
      file("misaki-us-gold", 1.5 * MIB, "Missing"),
    ],
    hearing_model: "base.en",
    hearing_files: [
      file("whisper-base.en", 140 * MIB, "Missing"),
      file("silero-vad", 2 * MIB, "Missing"),
    ],
    microphone: "Undetermined",
    mic_mode: "Open",
    input: { kind: "SystemDefault" },
    input_devices: [],
    harnesses: { claude_code: true, pi: false },
    blockers: [
      { kind: "TalkerKey", provider: "Claude" },
      { kind: "HearingNotReady" },
      { kind: "MicrophoneNotAllowed" },
    ],
  };
}

function ready(status: SetupStatus): SetupStatus {
  return {
    ...status,
    hearing_files: status.hearing_files.map((entry) => file(entry.id, entry.size_bytes, "Ready")),
  };
}

export const PRESETS = [
  "downloading",
  "ready",
  "key-saved",
  "mic-denied",
  "ollama",
  "settings",
] as const;
export type Preset = (typeof PRESETS)[number];

export function isPreset(name: string): name is Preset {
  return PRESETS.some((preset) => preset === name);
}

/**
 * The visual check's states: the canvas artboard (Claude picked, Kokoro at 201 of 312 MB,
 * Whisper ready, the microphone not asked), then everything ready, a saved key, a denied
 * microphone, and a local talker.
 */
export function presetTransport(preset: Preset): FakeSetupTransport {
  const base = ready(firstLaunch());
  const transport = new FakeSetupTransport(base);
  // Kokoro's model mid-download: its other files are small and land first.
  transport.initialEvents.set("kokoro", {
    kind: "Progress",
    received_bytes: 199 * MIB,
    total_bytes: 310 * MIB,
  });
  const smallReady = base.voice_files.map((entry) =>
    entry.id === "kokoro" ? entry : file(entry.id, entry.size_bytes, "Ready"),
  );
  const keySaved: SetupStatus = {
    ...base,
    voice_files: smallReady,
    talker: { ...base.talker, key: "Saved" },
    keys: ["Anthropic"],
    microphone: "Granted",
    blockers: [],
  };
  switch (preset) {
    case "downloading":
      transport.current = {
        ...base,
        voice_files: smallReady,
        blockers: [{ kind: "TalkerKey", provider: "Claude" }, { kind: "MicrophoneNotAllowed" }],
      };
      break;
    case "ready":
      transport.initialEvents.clear();
      transport.current = {
        ...keySaved,
        voice_files: base.voice_files.map((entry) => file(entry.id, entry.size_bytes, "Ready")),
      };
      break;
    case "key-saved":
      transport.current = keySaved;
      break;
    case "settings":
      transport.initialEvents.clear();
      transport.current = {
        ...keySaved,
        complete: true,
        voice_files: base.voice_files.map((entry) => file(entry.id, entry.size_bytes, "Ready")),
      };
      break;
    case "mic-denied":
      transport.current = {
        ...keySaved,
        microphone: "Denied",
        mic_mode: "PushToTalk",
        blockers: [{ kind: "MicrophoneDenied" }],
      };
      break;
    case "ollama":
      transport.current = {
        ...keySaved,
        talker: {
          provider: "Ollama",
          model: "gemma4:latest",
          key: "NotNeeded",
          ollama: { state: "Reachable", models: ["gemma4:latest", "qwen3.5:2b"] },
        },
        keys: [],
      };
      break;
  }
  return transport;
}
