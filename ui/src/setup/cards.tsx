// The three cards of artboard 11 · First run: who does the talking, the local voice and
// hearing models, and the microphone. They show the controller's state and send its actions;
// every value they show comes from the app's status.

import type { ComponentChildren } from "preact";
import type { InputDevice } from "../protocol/generated/InputDevice.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { SetupController } from "./controller.ts";
import { KeyField } from "./KeyField.tsx";
import { type GroupProgress, groupProgress, type SetupState } from "./state.ts";
import {
  groupSize,
  inputLabel,
  keyHint,
  keyLabel,
  keyNote,
  MODES,
  micDenied,
  micStep,
  OLLAMA_UNREACHABLE,
  OPEN_MIC_SETTINGS,
  PROVIDERS,
  percent,
  providerKey,
  thinkingStep,
  voiceStep,
} from "./words.ts";

export interface CardProps {
  readonly state: SetupState;
  readonly status: SetupStatus;
  readonly controller: SetupController;
}

function CardHead({ id, step, title }: { id: string; step: string; title: string }) {
  return (
    <div class="setup-card-head">
      <span class="setup-step">{step}</span>
      <h2 id={id} class="setup-card-title">
        {title}
      </h2>
    </div>
  );
}

export function Problem({ text }: { text: string | null }) {
  return text === null ? null : (
    <p class="setup-problem" role="status">
      {text}
    </p>
  );
}

function Foot({ children }: { children: ComponentChildren }) {
  return <div class="setup-foot">{children}</div>;
}

function OllamaModel({ status, controller, busy }: CardProps & { busy: boolean }) {
  const { model, ollama } = status.talker;
  const listed = ollama.state === "Reachable" ? ollama.models : [];
  const found = listed.includes(model);
  const note = found ? "found" : ollama.state === "Reachable" ? "not pulled" : "not running";
  const choices = found ? listed : [model, ...listed];
  return (
    <div class="setup-key">
      <span class="setup-label">Ollama at localhost:11434 · nothing leaves this Mac</span>
      <div class="setup-field setup-local">
        {choices.length > 1 ? (
          <select
            class="setup-select"
            aria-label="Ollama model"
            value={model}
            disabled={busy}
            onChange={(event) => void controller.pickOllamaModel(event.currentTarget.value)}
          >
            {choices.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        ) : (
          <span>{model}</span>
        )}
        <span class="setup-quiet">{note}</span>
      </div>
      {ollama.state === "Unreachable" ? <Problem text={OLLAMA_UNREACHABLE} /> : null}
    </div>
  );
}

export function ThinkingCard(props: CardProps) {
  const { state, status, controller } = props;
  const busy = state.busy.has("thinking");
  const provider = status.talker.provider;
  const key = providerKey(provider);
  const unreadable =
    status.talker.key === "Unreadable"
      ? "BoardAI couldn't read the Keychain. Unlock it, then come back."
      : null;
  return (
    <section class="setup-card" aria-labelledby="setup-think" aria-busy={busy}>
      <CardHead id="setup-think" step={thinkingStep(status)} title="Who does the talking" />
      <div role="radiogroup" aria-label="Language model provider" class="setup-pills">
        {PROVIDERS.map((choice) => (
          <label key={choice.id} class="setup-pill">
            <input
              type="radio"
              name="setup-provider"
              class="setup-radio"
              checked={choice.id === provider}
              disabled={busy}
              onChange={() => void controller.pickTalker(choice.id)}
            />
            {choice.label}
          </label>
        ))}
      </div>
      {key === null ? null : (
        <KeyField
          id="setup-key"
          label={keyLabel(key)}
          hint={keyHint(key, status.talker.key === "Saved")}
          note={keyNote(status.talker.key === "Saved")}
          disabled={busy}
          onSave={(value) => void controller.saveKey(key, value)}
        />
      )}
      {provider === "Ollama" ? <OllamaModel {...props} busy={busy} /> : null}
      {provider === "Other" ? (
        <p class="setup-note">
          settings.toml names another provider. Pick one here to replace it, or keep it.
        </p>
      ) : null}
      <Problem text={state.problems.thinking ?? unreadable} />
      {state.addingVoiceKey ? <VoiceKey {...props} /> : null}
      <Foot>
        <span class="setup-foot-label">Model</span>
        <span class="setup-mono">{status.talker.model}</span>
      </Foot>
    </section>
  );
}

/**
 * The ElevenLabs key, opened from the voice card's "Add a key": it sits with the other key in
 * card 1, under a rule (11b), and takes the focus when it opens.
 */
function VoiceKey({ state, status, controller }: CardProps) {
  return (
    <>
      <div class="setup-rule" />
      <KeyField
        id="setup-voice-key"
        label={keyLabel("ElevenLabs")}
        hint={keyHint("ElevenLabs", status.keys.includes("ElevenLabs"))}
        focus={true}
        disabled={state.busy.has("voice")}
        onSave={(value) => void controller.saveKey("ElevenLabs", value)}
      />
      <Problem text={state.problems.voice} />
    </>
  );
}

function ModelRow({
  name,
  label,
  progress,
}: {
  name: string;
  label: string;
  progress: GroupProgress;
}) {
  const value = percent(progress);
  const stopped = progress.problem !== null;
  return (
    <div class="setup-model">
      <div class="setup-model-head">
        <span class="setup-model-name">{name}</span>
        <span class="setup-mono setup-quiet setup-size">{groupSize(progress)}</span>
      </div>
      <div
        role="progressbar"
        aria-label={stopped ? `${label}, stopped` : label}
        aria-valuenow={value}
        aria-valuemin={0}
        aria-valuemax={100}
        class="setup-bar"
      >
        <div
          class={stopped ? "setup-bar-fill setup-bar-stopped" : "setup-bar-fill"}
          style={{ width: value === 100 ? "calc(100% - 2px)" : `${value}%` }}
        />
      </div>
    </div>
  );
}

export function VoiceCard({ state, status, controller }: CardProps) {
  const busy = state.busy.has("voice");
  const kokoro = status.voice === "Kokoro";
  const voice = groupProgress(status.voice_files, state.transfers);
  const hearing = groupProgress(status.hearing_files, state.transfers);
  const downloadProblem = hearing.problem ?? (kokoro ? voice.problem : null);
  const elevenLabsSaved = status.keys.includes("ElevenLabs");
  return (
    <section class="setup-card" aria-labelledby="setup-voice" aria-busy={busy}>
      <CardHead id="setup-voice" step={voiceStep(downloadProblem !== null)} title="Local models" />
      {kokoro ? (
        <ModelRow name="Voice · Kokoro" label="Kokoro voice download" progress={voice} />
      ) : (
        <div class="setup-model-head">
          <span class="setup-model-name">
            {status.voice === "ElevenLabs" ? "Voice · ElevenLabs" : "Voice · macOS"}
          </span>
          <span class="setup-mono setup-quiet setup-size">
            {status.voice === "ElevenLabs" ? "key saved" : "built in"}
          </span>
        </div>
      )}
      <ModelRow
        name={`Hearing · Whisper ${status.hearing_model}`}
        label="Whisper download"
        progress={hearing}
      />
      <p class="setup-note">
        Checked against SHA-256. Until the voice finishes, the macOS system voice speaks.
      </p>
      {downloadProblem === null ? null : (
        <div class="setup-retry">
          <Problem text={downloadProblem} />
          <button type="button" class="setup-again" onClick={() => controller.retryDownloads()}>
            Try again
          </button>
        </div>
      )}
      {/* While the key field is open, its refusal shows under it in card 1. */}
      <Problem text={state.addingVoiceKey ? null : state.problems.voice} />
      <Foot>
        <span class="setup-foot-label">
          {kokoro ? "Prefer ElevenLabs?" : "Prefer the local voice?"}
        </span>
        <button
          type="button"
          class="setup-small"
          disabled={busy}
          onClick={() => {
            if (!kokoro) {
              void controller.useVoice("Kokoro");
            } else if (elevenLabsSaved) {
              void controller.useVoice("ElevenLabs");
            } else {
              controller.openVoiceKey(!state.addingVoiceKey);
            }
          }}
        >
          {!kokoro
            ? "Use Kokoro"
            : elevenLabsSaved
              ? "Use ElevenLabs"
              : state.addingVoiceKey
                ? "Not now"
                : "Add a key"}
        </button>
      </Foot>
    </section>
  );
}

const SYSTEM_DEFAULT = "system_default";
const BUILT_IN = "built_in";

function inputValue(device: InputDevice): string {
  switch (device.kind) {
    case "SystemDefault":
      return SYSTEM_DEFAULT;
    case "BuiltIn":
      return BUILT_IN;
    case "Named":
      return `named:${device.name}`;
  }
}

function inputDevice(value: string): InputDevice {
  if (value === SYSTEM_DEFAULT) {
    return { kind: "SystemDefault" };
  }
  if (value === BUILT_IN) {
    return { kind: "BuiltIn" };
  }
  return { kind: "Named", name: value.slice("named:".length) };
}

function InputPicker({ status, controller, busy }: CardProps & { busy: boolean }) {
  const devices: InputDevice[] = [
    { kind: "SystemDefault" },
    { kind: "BuiltIn" },
    ...status.input_devices.map((name): InputDevice => ({ kind: "Named", name })),
  ];
  return (
    <label class="setup-input">
      Input
      <select
        class="setup-input-select"
        aria-label="Input device"
        value={inputValue(status.input)}
        disabled={busy}
        onChange={(event) => void controller.pickInput(inputDevice(event.currentTarget.value))}
      >
        {devices.map((device) => (
          <option key={inputValue(device)} value={inputValue(device)}>
            {inputLabel(device)}
          </option>
        ))}
      </select>
    </label>
  );
}

/** A check once allowed; the microphone to ask; slashed when macOS says no. */
function MicGlyph({ permission }: { permission: SetupStatus["microphone"] }) {
  return permission === "Granted" ? (
    <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24">
      <path d="M5 12.5l4.5 4.5L19 7.5" />
    </svg>
  ) : (
    <svg aria-hidden="true" width="22" height="22" viewBox="0 0 24 24">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5.5 11a6.5 6.5 0 0 0 13 0" />
      <path d="M12 17.5V21" />
      {permission === "Denied" ? <path d="M4 4l16 16" /> : null}
    </svg>
  );
}

export function MicrophoneCard(props: CardProps) {
  const { state, status, controller } = props;
  const busy = state.busy.has("microphone");
  const permission = status.microphone;
  return (
    <section class="setup-card" aria-labelledby="setup-mic" aria-busy={busy}>
      <CardHead id="setup-mic" step={micStep(permission)} title="How you speak to it" />
      <div class="setup-mic-row">
        {permission === "Undetermined" ? (
          <button
            type="button"
            class="setup-mic-ask"
            disabled={busy}
            onClick={() => void controller.allowMicrophone()}
          >
            <span class="setup-mic-glyph">
              <MicGlyph permission={permission} />
            </span>
            Allow microphone
          </button>
        ) : (
          <div class="setup-mic-state">
            <span
              class={
                permission === "Denied" ? "setup-mic-badge setup-mic-badge-off" : "setup-mic-badge"
              }
            >
              <MicGlyph permission={permission} />
            </span>
            <span>{permission === "Granted" ? "Microphone allowed" : "Microphone off"}</span>
          </div>
        )}
        <InputPicker {...props} busy={busy} />
      </div>
      {permission === "Denied" ? (
        <div class="setup-mic-denied">
          <Problem text={micDenied(status.mic_attributed_to)} />
          <button
            type="button"
            class="setup-again"
            onClick={() => void controller.openMicrophoneSettings()}
          >
            {OPEN_MIC_SETTINGS}
          </button>
        </div>
      ) : null}
      <Problem text={state.problems.microphone} />
      <div role="radiogroup" aria-label="Microphone mode" class="setup-modes">
        {MODES.map((mode) => (
          <label key={mode.id} class="setup-mode">
            <input
              type="radio"
              name="setup-mic-mode"
              class="setup-radio"
              checked={mode.id === status.mic_mode}
              disabled={busy}
              onChange={() => void controller.pickMode(mode.id)}
            />
            <span class="setup-mode-label">{mode.label}</span>
            <span class="setup-note">{mode.note}</span>
          </label>
        ))}
      </div>
    </section>
  );
}
