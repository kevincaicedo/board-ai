// The setup screens' trust boundary (LLD §3.8): what the app answers, checked once here into the
// generated types, field by field as protocol/parse.ts checks the session's events. A shell from
// another build must not hand the screen a state no branch handles: a status that doesn't parse
// is `null`, and the screen says the versions don't match. A model or provider row that doesn't
// parse is left out and the rest still show (D-119); a median speed is a number or unknown,
// never made up (D-176).

import {
  type Fields,
  fields,
  flag,
  integer,
  list,
  nullable,
  oneOf,
  optional,
  type Tagged,
  tagged,
  text,
  U16_MAX,
  u32,
} from "../protocol/fields.ts";
import type { DownloadEvent } from "../protocol/generated/DownloadEvent.ts";
import type { DownloadProblem } from "../protocol/generated/DownloadProblem.ts";
import type { Drawing } from "../protocol/generated/Drawing.ts";
import type { DrawingMeasure } from "../protocol/generated/DrawingMeasure.ts";
import type { ElevenLabsVoice } from "../protocol/generated/ElevenLabsVoice.ts";
import type { FileState } from "../protocol/generated/FileState.ts";
import type { HarnessesFound } from "../protocol/generated/HarnessesFound.ts";
import type { HarnessFound } from "../protocol/generated/HarnessFound.ts";
import type { HarnessName } from "../protocol/generated/HarnessName.ts";
import type { IndexRebuilt } from "../protocol/generated/IndexRebuilt.ts";
import type { InputDevice } from "../protocol/generated/InputDevice.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { KeyState } from "../protocol/generated/KeyState.ts";
import type { KeyStatus } from "../protocol/generated/KeyStatus.ts";
import type { MicChoice } from "../protocol/generated/MicChoice.ts";
import type { MicPermission } from "../protocol/generated/MicPermission.ts";
import type { ModelFileStatus } from "../protocol/generated/ModelFileStatus.ts";
import type { ModelInfo } from "../protocol/generated/ModelInfo.ts";
import type { ModelListing } from "../protocol/generated/ModelListing.ts";
import type { ModelPrice } from "../protocol/generated/ModelPrice.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { ModelReasoning } from "../protocol/generated/ModelReasoning.ts";
import type { OllamaStatus } from "../protocol/generated/OllamaStatus.ts";
import type { SetupBlocker } from "../protocol/generated/SetupBlocker.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TalkerProvider } from "../protocol/generated/TalkerProvider.ts";
import type { TalkerSetup } from "../protocol/generated/TalkerSetup.ts";
import type { ThinkingEffort } from "../protocol/generated/ThinkingEffort.ts";
import type { VoiceEngine } from "../protocol/generated/VoiceEngine.ts";

const KEY_NAMES: readonly KeyName[] = ["Anthropic", "OpenRouter", "ElevenLabs"];
const KEY_STATES: readonly KeyState[] = [
  "NotNeeded",
  "Saved",
  "Missing",
  "Unreadable",
  "NotChecked",
];
const PROVIDERS: readonly TalkerProvider[] = ["Claude", "OpenRouter", "Ollama", "Other"];
const EFFORTS: readonly ThinkingEffort[] = ["Off", "Low", "Medium", "High", "ExtraHigh", "Max"];
const FILE_STATES: readonly FileState[] = ["Missing", "Partial", "Downloading", "Ready"];
const VOICES: readonly VoiceEngine[] = ["Kokoro", "ElevenLabs", "System"];
const PERMISSIONS: readonly MicPermission[] = ["Undetermined", "Denied", "Granted"];
const MIC_CHOICES: readonly MicChoice[] = ["Open", "PushToTalk"];
const HARNESS_NAMES: readonly HarnessName[] = ["ClaudeCode", "Pi"];

/** A u64 on the wire (bytes, counts, prices): JavaScript numbers are exact up to 2^53. */
const u64 = (value: unknown): number | null => integer(value, 0, Number.MAX_SAFE_INTEGER);

/** A list of strings: any other entry makes the whole list malformed. */
const texts = (value: unknown): string[] | null => list(value, text);

const OLLAMA: Tagged<OllamaStatus, "state"> = {
  Unreachable: () => ({ state: "Unreachable" }),
  Reachable: (f) => {
    const models = texts(f.models);
    return models === null ? null : { state: "Reachable", models };
  },
};

const HARNESS: Tagged<HarnessFound, "state"> = {
  Missing: () => ({ state: "Missing" }),
  Found: (f) => {
    const version = nullable(f.version, text);
    return version === undefined ? null : { state: "Found", version };
  },
};

const INPUTS: Tagged<InputDevice, "kind"> = {
  SystemDefault: () => ({ kind: "SystemDefault" }),
  BuiltIn: () => ({ kind: "BuiltIn" }),
  Named: (f) => {
    const name = text(f.name);
    return name === null ? null : { kind: "Named", name };
  },
};

/** A blocker that names a model or a provider. */
function named<K extends string>(kind: K, f: Fields): { kind: K; model: string } | null {
  const model = text(f.model);
  return model === null ? null : { kind, model };
}

const BLOCKERS: Tagged<SetupBlocker, "kind"> = {
  SettingsProblem: () => ({ kind: "SettingsProblem" }),
  TalkerKey: (f) => {
    const provider = oneOf(f.provider, PROVIDERS);
    return provider === null ? null : { kind: "TalkerKey", provider };
  },
  OllamaUnreachable: () => ({ kind: "OllamaUnreachable" }),
  OllamaModelMissing: (f) => named("OllamaModelMissing", f),
  UnknownHearingModel: (f) => named("UnknownHearingModel", f),
  HearingNotReady: () => ({ kind: "HearingNotReady" }),
  MicrophoneNotAllowed: () => ({ kind: "MicrophoneNotAllowed" }),
  MicrophoneDenied: () => ({ kind: "MicrophoneDenied" }),
};

const REASONING: Tagged<ModelReasoning, "kind"> = {
  Unknown: () => ({ kind: "Unknown" }),
  Never: () => ({ kind: "Never" }),
  Optional: () => ({ kind: "Optional" }),
  Always: (f) => {
    const least = text(f.least_effort);
    return least === null ? null : { kind: "Always", least_effort: least };
  },
};

function measure(value: unknown): DrawingMeasure | null {
  const f = fields(value);
  const drew = u32(f?.drew);
  const answers = u32(f?.answers);
  const firstStroke = u32(f?.first_stroke_ms_p50);
  const measuredOn = text(f?.measured_on);
  return drew === null ||
    answers === null ||
    answers === 0 ||
    drew > answers ||
    firstStroke === null ||
    measuredOn === null
    ? null
    : { drew, answers, first_stroke_ms_p50: firstStroke, measured_on: measuredOn };
}

const DRAWING: Tagged<Drawing, "kind"> = {
  NotMeasured: () => ({ kind: "NotMeasured" }),
  Checked: (f) => {
    const measured = measure(f.measure);
    return measured === null ? null : { kind: "Checked", measure: measured };
  },
  Sometimes: (f) => {
    const measured = measure(f.measure);
    return measured === null ? null : { kind: "Sometimes", measure: measured };
  },
};

/** How a model draws (D-219). A shell from before it sends none, which reads as not measured. */
function drawing(value: unknown): Drawing | null {
  return value === undefined ? { kind: "NotMeasured" } : tagged<Drawing>(DRAWING, "kind", value);
}

function talker(value: unknown): TalkerSetup | null {
  const f = fields(value);
  if (f === null) {
    return null;
  }
  const provider = oneOf(f.provider, PROVIDERS);
  const model = text(f.model);
  const key = oneOf(f.key, KEY_STATES);
  const ollama = tagged<OllamaStatus>(OLLAMA, "state", f.ollama);
  const thinking = oneOf(f.thinking, EFFORTS);
  const route = texts(f.route);
  const preset = flag(f.preset);
  const draws = drawing(f.drawing);
  return provider === null ||
    model === null ||
    key === null ||
    ollama === null ||
    thinking === null ||
    route === null ||
    preset === null ||
    draws === null
    ? null
    : { provider, model, key, ollama, thinking, route, preset, drawing: draws };
}

function keyStatus(value: unknown): KeyStatus | null {
  const f = fields(value);
  const name = oneOf(f?.name, KEY_NAMES);
  const state = oneOf(f?.state, KEY_STATES);
  return name === null || state === null ? null : { name, state };
}

function fileStatus(value: unknown): ModelFileStatus | null {
  const f = fields(value);
  const id = text(f?.id);
  const state = oneOf(f?.state, FILE_STATES);
  const have = u64(f?.have_bytes);
  const size = u64(f?.size_bytes);
  return id === null || state === null || have === null || size === null
    ? null
    : { id, state, have_bytes: have, size_bytes: size };
}

function elevenLabs(value: unknown): ElevenLabsVoice | null {
  const f = fields(value);
  const model = text(f?.model);
  const voice = text(f?.voice);
  return model === null || voice === null ? null : { model, voice };
}

function harnesses(value: unknown): HarnessesFound | null {
  const f = fields(value);
  if (f === null) {
    return null;
  }
  const claudeCode = tagged<HarnessFound>(HARNESS, "state", f.claude_code);
  const pi = tagged<HarnessFound>(HARNESS, "state", f.pi);
  const chosen = nullable(f.default, (name) => oneOf(name, HARNESS_NAMES));
  return claudeCode === null || pi === null || chosen === undefined
    ? null
    : { claude_code: claudeCode, pi, default: chosen };
}

/** The app macOS asks about the microphone; a shell from before the field (D-149) sends none. */
function attributedTo(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

/** The status as the app sent it, or `null` when any field doesn't parse. */
export function parseSetupStatus(value: unknown): SetupStatus | null {
  const f = fields(value);
  if (f === null) {
    return null;
  }
  const done = flag(f.complete);
  const settingsProblem = nullable(f.settings_problem, text);
  const who = talker(f.talker);
  const scribe = text(f.scribe_model);
  const keys = list(f.keys, keyStatus);
  const voice = oneOf(f.voice, VOICES);
  const eleven = elevenLabs(f.elevenlabs);
  const voiceFiles = list(f.voice_files, fileStatus);
  const hearingModel = text(f.hearing_model);
  const hearingFiles = list(f.hearing_files, fileStatus);
  const microphone = oneOf(f.microphone, PERMISSIONS);
  const micMode = oneOf(f.mic_mode, MIC_CHOICES);
  const input = tagged<InputDevice>(INPUTS, "kind", f.input);
  const inputs = texts(f.input_devices);
  const defaultInput = nullable(f.default_input, text);
  const builtIn = nullable(f.built_in_input, text);
  const found = harnesses(f.harnesses);
  const folders = texts(f.code_folders);
  const vault = text(f.vault);
  const blockers = list(f.blockers, (blocker) => tagged<SetupBlocker>(BLOCKERS, "kind", blocker));
  if (
    done === null ||
    settingsProblem === undefined ||
    who === null ||
    scribe === null ||
    keys === null ||
    voice === null ||
    eleven === null ||
    voiceFiles === null ||
    hearingModel === null ||
    hearingFiles === null ||
    microphone === null ||
    micMode === null ||
    input === null ||
    inputs === null ||
    defaultInput === undefined ||
    builtIn === undefined ||
    found === null ||
    folders === null ||
    vault === null ||
    blockers === null
  ) {
    return null;
  }
  return {
    complete: done,
    settings_problem: settingsProblem,
    talker: who,
    scribe_model: scribe,
    keys,
    voice,
    elevenlabs: eleven,
    voice_files: voiceFiles,
    hearing_model: hearingModel,
    hearing_files: hearingFiles,
    microphone,
    mic_mode: micMode,
    input,
    input_devices: inputs,
    default_input: defaultInput,
    built_in_input: builtIn,
    harnesses: found,
    code_folders: folders,
    vault,
    blockers,
    mic_attributed_to: attributedTo(f.mic_attributed_to),
  };
}

function price(value: unknown): ModelPrice | null {
  const f = fields(value);
  const input = u64(f?.input_micro_usd_per_million_tokens);
  const output = u64(f?.output_micro_usd_per_million_tokens);
  return input === null || output === null
    ? null
    : { input_micro_usd_per_million_tokens: input, output_micro_usd_per_million_tokens: output };
}

/** A model row, or `null`. Fields a shell from before them doesn't send read as none. */
function model(value: unknown): ModelInfo | null {
  const f = fields(value);
  if (f === null) {
    return null;
  }
  const id = text(f.id);
  const name = text(f.name);
  const context = optional(f.context_tokens, u32);
  const cost = optional(f.price, price);
  const reasoning = tagged<ModelReasoning>(REASONING, "kind", f.reasoning);
  const preset = flag(f.preset);
  const route = f.route === undefined ? [] : texts(f.route);
  const draws = drawing(f.drawing);
  return id === null ||
    name === null ||
    context === undefined ||
    cost === undefined ||
    reasoning === null ||
    preset === null ||
    route === null ||
    draws === null
    ? null
    : { id, name, context_tokens: context, price: cost, reasoning, preset, route, drawing: draws };
}

/** A median OpenRouter may not give: a number, or unknown (never made up, D-176). */
function median(value: unknown): number | null {
  return u32(value);
}

function provider(value: unknown): ModelProvider | null {
  const f = fields(value);
  if (f === null) {
    return null;
  }
  const name = text(f.name);
  const slug = text(f.slug);
  const context = optional(f.context_tokens, u32);
  const output = optional(f.max_output_tokens, u32);
  const cost = optional(f.price, price);
  const quantization = optional(f.quantization, text);
  return name === null ||
    slug === null ||
    context === undefined ||
    output === undefined ||
    cost === undefined ||
    quantization === undefined
    ? null
    : {
        name,
        slug,
        context_tokens: context,
        max_output_tokens: output,
        price: cost,
        quantization,
        tools: f.tools === true,
        latency_ms_p50: median(f.latency_ms_p50),
        throughput_tokens_per_second_p50: median(f.throughput_tokens_per_second_p50),
      };
}

/** Rows that parse; the others are left out and the rest still show. */
function rows<T>(values: readonly unknown[], parse: (value: unknown) => T | null): T[] {
  const out: T[] = [];
  for (const value of values) {
    const row = parse(value);
    if (row !== null) {
      out.push(row);
    }
  }
  return out;
}

/** A model list as the app sent it: its kind and arrays, then each row on its own. */
export function parseModelListing(value: unknown): ModelListing | null {
  const f = fields(value);
  if (f === null) {
    return null;
  }
  // A shell from before the count sends none: nothing was left out that it knows of.
  const leftOut = f.left_out === undefined ? 0 : u32(f.left_out);
  if (leftOut === null) {
    return null;
  }
  const models = f.models;
  switch (f.kind) {
    case "OpenRouter":
      return Array.isArray(models)
        ? { kind: "OpenRouter", models: rows(models, model), left_out: leftOut }
        : null;
    case "Ollama":
      return Array.isArray(models) ? { kind: "Ollama", models: rows(models, model) } : null;
    case "Providers": {
      const name = text(f.model);
      const providers = f.providers;
      return name !== null && Array.isArray(providers)
        ? {
            kind: "Providers",
            model: name,
            providers: rows(providers, provider),
            left_out: leftOut,
          }
        : null;
    }
    default:
      return null;
  }
}

/** What "Rebuild the index" answered, or `null`. */
export function parseIndexRebuilt(value: unknown): IndexRebuilt | null {
  const f = fields(value);
  const seen = u64(f?.files_seen);
  const reindexed = u64(f?.reindexed);
  const skipped = u64(f?.skipped);
  const truncated = flag(f?.truncated);
  return seen === null || reindexed === null || skipped === null || truncated === null
    ? null
    : { files_seen: seen, reindexed, skipped, truncated };
}

const PROBLEMS: Tagged<DownloadProblem, "kind"> = {
  Network: () => ({ kind: "Network" }),
  Server: (f) => {
    const status = integer(f.status, 0, U16_MAX);
    return status === null ? null : { kind: "Server", status };
  },
  Unexpected: () => ({ kind: "Unexpected" }),
  Corrupt: () => ({ kind: "Corrupt" }),
  Disk: () => ({ kind: "Disk" }),
};

const DOWNLOADS: Tagged<DownloadEvent, "kind"> = {
  Progress: (f) => {
    const received = u64(f.received_bytes);
    const total = u64(f.total_bytes);
    return received === null || total === null
      ? null
      : { kind: "Progress", received_bytes: received, total_bytes: total };
  },
  Done: () => ({ kind: "Done" }),
  Failed: (f) => {
    const problem = tagged<DownloadProblem>(PROBLEMS, "kind", f.problem);
    return problem === null ? null : { kind: "Failed", problem };
  },
};

/** One download event from the app's channel, or `null` (a mismatched build's). */
export function parseDownloadEvent(value: unknown): DownloadEvent | null {
  return tagged<DownloadEvent>(DOWNLOADS, "kind", value);
}
