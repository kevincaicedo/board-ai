// An in-memory SetupTransport for tests and the visual check (`?setup=<preset>` outside Tauri).
// It answers with statuses a test or preset gives it; it doesn't decide blockers itself (the
// app does), so a test states the status it expects the app to send.

import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { KeyState } from "../protocol/generated/KeyState.ts";
import type { KeyStatus } from "../protocol/generated/KeyStatus.ts";
import type { ModelFileStatus } from "../protocol/generated/ModelFileStatus.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelQuery } from "../protocol/generated/ModelQuery.ts";
import type { SetupEdit } from "../protocol/generated/SetupEdit.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { recordedListing } from "./fakeModels.ts";
import type { SetupTransport } from "./transport.ts";

export type FakeCall =
  | { readonly kind: "status" }
  | { readonly kind: "save"; readonly edit: SetupEdit }
  | { readonly kind: "saveKey"; readonly name: KeyName; readonly length: number }
  | { readonly kind: "download"; readonly model: string }
  | { readonly kind: "requestMicrophone" }
  | { readonly kind: "openMicrophoneSettings" }
  | { readonly kind: "finish" }
  | { readonly kind: "models"; readonly query: ModelQuery }
  | { readonly kind: "removeKey"; readonly name: KeyName }
  | { readonly kind: "checkKeys" }
  | { readonly kind: "harnessVersions" }
  | { readonly kind: "addCodeFolder" }
  | { readonly kind: "openVault" }
  | { readonly kind: "rebuildIndex" };

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
  onRemoveKey: (name: KeyName) => SetupStatus = (name) => withKey(this.current, name, "Missing");
  onCheckKeys: () => SetupStatus = () => this.current;
  onHarnessVersions: () => SetupStatus = () => this.current;
  onAddCodeFolder: () => SetupStatus = () => this.current;
  onModels: (query: ModelQuery) => ModelListing = recordedListing;
  rebuilt: IndexRebuilt = { files_seen: 128, reindexed: 128, skipped: 0, truncated: false };
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

  async openMicrophoneSettings(): Promise<void> {
    this.#record({ kind: "openMicrophoneSettings" });
  }

  async finish(): Promise<void> {
    this.#record({ kind: "finish" });
  }

  async models(query: ModelQuery): Promise<ModelListing> {
    this.#record({ kind: "models", query });
    return this.onModels(query);
  }

  async removeKey(name: KeyName): Promise<SetupStatus> {
    this.#record({ kind: "removeKey", name });
    this.current = this.onRemoveKey(name);
    return this.current;
  }

  async checkKeys(): Promise<SetupStatus> {
    this.#record({ kind: "checkKeys" });
    this.current = this.onCheckKeys();
    return this.current;
  }

  async harnessVersions(): Promise<SetupStatus> {
    this.#record({ kind: "harnessVersions" });
    this.current = this.onHarnessVersions();
    return this.current;
  }

  async addCodeFolder(): Promise<SetupStatus> {
    this.#record({ kind: "addCodeFolder" });
    this.current = this.onAddCodeFolder();
    return this.current;
  }

  async openVault(): Promise<void> {
    this.#record({ kind: "openVault" });
  }

  async rebuildIndex(): Promise<IndexRebuilt> {
    this.#record({ kind: "rebuildIndex" });
    return this.rebuilt;
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

const KEYS: readonly KeyName[] = ["Anthropic", "OpenRouter", "ElevenLabs"];

/** Every key in the app's order, `saved` ones saved and the rest in `rest`. */
export function keyStates(saved: readonly KeyName[], rest: KeyState = "Missing"): KeyStatus[] {
  return KEYS.map((name) => ({ name, state: saved.includes(name) ? "Saved" : rest }));
}

/** The status with one key's state changed, as the app would answer. */
export function withKey(status: SetupStatus, name: KeyName, state: KeyState): SetupStatus {
  return {
    ...status,
    keys: status.keys.map((key) => (key.name === name ? { name, state } : key)),
  };
}

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
      thinking: "Low",
      route: [],
      preset: false,
    },
    scribe_model: "claude-haiku-5-5",
    keys: keyStates([]),
    voice: "Kokoro",
    elevenlabs: { model: "Flash", voice: "Rachel" },
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
    default_input: null,
    built_in_input: null,
    harnesses: {
      claude_code: { state: "Found", version: null },
      pi: { state: "Missing" },
      default: "ClaudeCode",
    },
    code_folders: [],
    vault: "~/BoardAI",
    mic_attributed_to: null,
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

/** A device name longer than the picker: it is cut with an ellipsis, never over the status. */
const LONG_DEVICE = "Jabra Evolve2 65 Flex USB-C Wireless Headset Microphone";

export const PRESETS = [
  "downloading",
  "ready",
  "key-saved",
  "mic-denied",
  "mic-elsewhere",
  "mic-long-name",
  "ollama",
  "settings",
  "settings-unchecked",
  "settings-ollama",
  "settings-offline",
  "download-error",
] as const;
export type Preset = (typeof PRESETS)[number];

export function isPreset(name: string): name is Preset {
  return PRESETS.some((preset) => preset === name);
}

/**
 * Artboard 12's settings page over a running session: OpenRouter's gpt-oss-120b routed to
 * Cerebras, OpenRouter's and ElevenLabs' keys saved, Kokoro ready, the Shure as macOS's default
 * input, the microphone allowed as Visual Studio Code (`just dev`), Claude Code found (its
 * version comes when the coding section asks), one code folder.
 */
function settingsStatus(base: SetupStatus): SetupStatus {
  return {
    ...base,
    complete: true,
    talker: {
      provider: "OpenRouter",
      model: "openai/gpt-oss-120b",
      key: "Saved",
      ollama: { state: "Reachable", models: ["gemma4:latest", "qwen3.5:2b"] },
      thinking: "Low",
      route: ["cerebras"],
      preset: true,
    },
    scribe_model: "qwen/qwen3.8-27b",
    keys: keyStates(["OpenRouter", "ElevenLabs"]),
    voice_files: base.voice_files.map((entry) => file(entry.id, entry.size_bytes, "Ready")),
    microphone: "Granted",
    mic_attributed_to: "Visual Studio Code",
    input_devices: ["Shure MV7+", "MacBook Pro Microphone", "Studio Display Microphone"],
    default_input: "Shure MV7+",
    built_in_input: "MacBook Pro Microphone",
    code_folders: ["~/Documents/Projects"],
    blockers: [],
  };
}

/**
 * The visual check's states: the canvas artboard (Claude picked, Kokoro at 201 of 312 MB,
 * Whisper ready, the microphone not asked), then everything ready, a saved key, a denied
 * microphone, a local talker, 11b's stopped Whisper download (61 of 142 MB, no connection) with
 * Start waiting for it and the microphone, and the settings page (artboard 12): as drawn, with
 * keys not checked this run, on Ollama, and with OpenRouter's list not arriving.
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
    keys: keyStates(["Anthropic"]),
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
    case "settings-unchecked":
    case "settings-offline": {
      transport.initialEvents.clear();
      const settings = settingsStatus(base);
      transport.current =
        preset === "settings-unchecked"
          ? {
              ...settings,
              keys: keyStates([], "NotChecked"),
              talker: { ...settings.talker, key: "NotChecked" },
            }
          : settings;
      transport.onCheckKeys = () => settings;
      transport.onHarnessVersions = () => ({
        ...transport.current,
        harnesses: { ...base.harnesses, claude_code: { state: "Found", version: "2.1.301" } },
      });
      transport.onAddCodeFolder = () => ({
        ...transport.current,
        code_folders: [...transport.current.code_folders, "~/code/boardai"],
      });
      if (preset === "settings-offline") {
        transport.refuse.models =
          "OpenRouter's list didn't arrive. Check the connection and try again.";
      }
      break;
    }
    case "settings-ollama":
      transport.initialEvents.clear();
      transport.current = {
        ...settingsStatus(base),
        talker: {
          provider: "Ollama",
          model: "gemma4:latest",
          key: "NotNeeded",
          ollama: { state: "Reachable", models: ["gemma4:latest", "qwen3.5:2b"] },
          thinking: "Off",
          route: [],
          preset: true,
        },
        scribe_model: "gemma4:latest",
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
    // `just dev` in an editor's terminal: macOS asks about the editor (D-149).
    case "mic-elsewhere":
      transport.current = {
        ...keySaved,
        microphone: "Denied",
        input: { kind: "Named", name: LONG_DEVICE },
        input_devices: [LONG_DEVICE, "MacBook Pro Microphone"],
        mic_attributed_to: "Visual Studio Code",
        blockers: [{ kind: "MicrophoneDenied" }],
      };
      break;
    case "mic-long-name":
      transport.current = {
        ...keySaved,
        input: { kind: "Named", name: LONG_DEVICE },
        input_devices: [LONG_DEVICE, "MacBook Pro Microphone"],
      };
      break;
    case "download-error":
      transport.initialEvents.clear();
      transport.initialEvents.set("whisper-base.en", {
        kind: "Failed",
        problem: { kind: "Network" },
      });
      transport.current = {
        ...keySaved,
        voice_files: base.voice_files.map((entry) => file(entry.id, entry.size_bytes, "Ready")),
        hearing_files: [
          { ...file("whisper-base.en", 140 * MIB, "Partial"), have_bytes: 59 * MIB },
          file("silero-vad", 2 * MIB, "Ready"),
        ],
        microphone: "Undetermined",
        blockers: [{ kind: "HearingNotReady" }, { kind: "MicrophoneNotAllowed" }],
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
          thinking: "Off",
          route: [],
          preset: true,
        },
        scribe_model: "gemma4:latest",
        keys: keyStates([]),
      };
      break;
  }
  return transport;
}
