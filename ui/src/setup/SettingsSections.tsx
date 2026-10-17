// The settings page's other sections (artboard 12 · Settings): the voice, the microphone, the
// keys, coding, and memory and privacy. They show the controller's state and send its actions;
// every value comes from the app's status.

import { useState } from "preact/hooks";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { VoiceEngine } from "../protocol/generated/VoiceEngine.ts";
import { Problem } from "./cards.tsx";
import type { SetupController } from "./controller.ts";
import { KeyField } from "./KeyField.tsx";
import {
  ALLOW_MICROPHONE,
  elevenLabsPickable,
  harnessLine,
  inputRows,
  KEYS_NOTE,
  keyHintFor,
  keyRow,
  keyStateOf,
  NO_CODE_FOLDERS,
  OPEN_MACOS_SETTINGS,
  permissionHelp,
  permissionLine,
  rebuiltLine,
  sameInput,
  type Tone,
  voiceName,
  voiceNote,
  voiceStatus,
} from "./settingsWords.ts";
import { groupProgress, type SetupState } from "./state.ts";
import { groupSize, KEY_NOTE, keyLabel, MODES, percent } from "./words.ts";

interface SectionProps {
  readonly state: SetupState;
  readonly status: SetupStatus;
  readonly controller: SetupController;
}

export function SectionHead({ id, step, title }: { id: string; step: string; title: string }) {
  return (
    <div class="settings-section-head">
      <span class="setup-step">{step}</span>
      <h2 id={id} class="settings-section-title">
        {title}
      </h2>
    </div>
  );
}

function Dot({ tone }: { tone: Tone }) {
  return <span aria-hidden="true" class={`settings-dot settings-dot-${tone}`} />;
}

const VOICES: readonly VoiceEngine[] = ["Kokoro", "ElevenLabs", "System"];

export function VoiceSection({
  state,
  status,
  controller,
  onAddKey,
}: SectionProps & { onAddKey: (key: KeyName) => void }) {
  const busy = state.busy.has("voice");
  const kokoro = groupProgress(status.voice_files, state.transfers);
  const hearing = groupProgress(status.hearing_files, state.transfers);
  const stopped = hearing.problem ?? (status.voice === "Kokoro" ? kokoro.problem : null);
  return (
    <section
      id="settings-voice"
      class="settings-section"
      aria-labelledby="settings-voice-title"
      aria-busy={busy}
    >
      <SectionHead id="settings-voice-title" step="VOICE" title="How it sounds" />
      <div role="radiogroup" aria-label="Voice" class="settings-voices">
        {VOICES.map((engine) => {
          const pickable = engine !== "ElevenLabs" || elevenLabsPickable(status);
          const downloading = engine === "Kokoro" && !kokoro.ready && kokoro.receivedBytes > 0;
          return (
            <label key={engine} class="settings-voice">
              <input
                type="radio"
                name="settings-voice"
                class="setup-radio"
                checked={status.voice === engine}
                disabled={busy || !pickable}
                onChange={() => void controller.useVoice(engine)}
              />
              <span class="settings-voice-name">{voiceName(engine)}</span>
              <span class="setup-note">{voiceNote(engine)}</span>
              {downloading ? (
                <span
                  role="progressbar"
                  aria-label="Kokoro voice download"
                  aria-valuenow={percent(kokoro)}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  class="setup-bar"
                >
                  <span
                    class={
                      kokoro.problem === null
                        ? "setup-bar-fill"
                        : "setup-bar-fill setup-bar-stopped"
                    }
                    style={{ width: `${percent(kokoro)}%` }}
                  />
                </span>
              ) : null}
              <span class="settings-mono-quiet">{voiceStatus(engine, status, kokoro)}</span>
              {pickable ? null : (
                <button
                  type="button"
                  class="settings-small settings-voice-key"
                  onClick={() => onAddKey("ElevenLabs")}
                >
                  Add a key
                </button>
              )}
            </label>
          );
        })}
      </div>
      <p class="settings-hearing">
        <span>Hearing · Whisper {status.hearing_model}</span>
        <span class="settings-mono-quiet">
          {hearing.problem === null ? groupSize(hearing) : "download stopped"}
        </span>
      </p>
      {stopped === null ? null : (
        <div class="setup-retry">
          <Problem text={stopped} />
          <button type="button" class="setup-again" onClick={() => controller.retryDownloads()}>
            Try again
          </button>
        </div>
      )}
      <Problem text={state.problems.voice} />
    </section>
  );
}

export function MicrophoneSection({ state, status, controller }: SectionProps) {
  const busy = state.busy.has("microphone");
  const permission = permissionLine(status);
  const help = permissionHelp(status);
  return (
    <section
      id="settings-microphone"
      class="settings-section"
      aria-labelledby="settings-microphone-title"
      aria-busy={busy}
    >
      <SectionHead id="settings-microphone-title" step="MICROPHONE" title="How you speak to it" />
      <div class="settings-halves">
        <div class="settings-stack">
          <span class="settings-label">Input</span>
          <div role="radiogroup" aria-label="Input device" class="settings-list">
            {inputRows(status).map((row) => (
              <label key={`${row.device.kind}:${row.name}`} class="settings-list-row">
                <input
                  type="radio"
                  name="settings-input"
                  class="setup-radio"
                  checked={sameInput(row.device, status.input)}
                  disabled={busy}
                  onChange={() => void controller.pickInput(row.device)}
                />
                <span class="settings-radio" aria-hidden="true" />
                <span class="settings-list-text">
                  <span class="settings-list-name">{row.name}</span>
                  {row.note === "" ? null : <span class="setup-note">{row.note}</span>}
                </span>
              </label>
            ))}
          </div>
        </div>
        <div class="settings-stack">
          <span class="settings-label">Mode</span>
          <div role="radiogroup" aria-label="Microphone mode" class="settings-modes">
            {MODES.map((mode) => (
              <label key={mode.id} class="settings-mode">
                <input
                  type="radio"
                  name="settings-mic-mode"
                  class="setup-radio"
                  checked={mode.id === status.mic_mode}
                  disabled={busy}
                  onChange={() => void controller.pickMode(mode.id)}
                />
                <span class="settings-mode-label">{mode.label}</span>
                <span class="setup-note">{mode.note}</span>
              </label>
            ))}
          </div>
          <span class="settings-label settings-label-gap">Permission</span>
          <div class="settings-permission">
            <span class="settings-permission-state">
              <Dot tone={permission.tone} />
              {permission.text}
            </span>
            {status.microphone === "Undetermined" ? (
              <button
                type="button"
                class="settings-small"
                disabled={busy}
                onClick={() => void controller.allowMicrophone()}
              >
                {ALLOW_MICROPHONE}
              </button>
            ) : (
              <button
                type="button"
                class="settings-small"
                onClick={() => void controller.openMicrophoneSettings()}
              >
                {OPEN_MACOS_SETTINGS}
              </button>
            )}
          </div>
          {help === null ? null : <p class="setup-problem">{help}</p>}
          <Problem text={state.problems.microphone} />
        </div>
      </div>
    </section>
  );
}

const KEYS: readonly KeyName[] = ["Anthropic", "OpenRouter", "ElevenLabs"];

export function KeysSection({
  state,
  status,
  controller,
  openKey,
  onOpenKey,
}: SectionProps & { openKey: KeyName | null; onOpenKey: (key: KeyName | null) => void }) {
  const busy = state.busy.has("keys");
  // "Remove" asks once more before it deletes.
  const [removing, setRemoving] = useState<KeyName | null>(null);
  return (
    <section
      id="settings-keys"
      class="settings-section"
      aria-labelledby="settings-keys-title"
      aria-busy={busy}
    >
      <div class="settings-section-top">
        <SectionHead id="settings-keys-title" step="KEYS" title="Your accounts" />
        <span class="settings-aside">{KEYS_NOTE}</span>
      </div>
      <ul aria-label="Keys" class="settings-keys">
        {KEYS.map((name) => {
          const keyState = keyStateOf(status, name);
          const row = keyRow(keyState);
          const open = openKey === name;
          return (
            <li key={name} class="settings-key">
              <div class="settings-key-row">
                <span class="settings-key-name">{name}</span>
                <span class="settings-key-state">
                  <Dot tone={row.tone} />
                  {row.status}
                </span>
                <span class="settings-key-actions">
                  <button
                    type="button"
                    class="settings-small"
                    disabled={busy}
                    aria-expanded={open}
                    onClick={() => {
                      setRemoving(null);
                      onOpenKey(open ? null : name);
                    }}
                  >
                    {open ? "Cancel" : row.primary}
                  </button>
                  {row.secondary === null ? null : (
                    <button
                      type="button"
                      class="settings-quiet-button"
                      disabled={busy}
                      onClick={() => {
                        if (row.secondary === "Check") {
                          void controller.checkKeys();
                        } else if (removing === name) {
                          setRemoving(null);
                          void controller.removeKey(name);
                        } else {
                          setRemoving(name);
                        }
                      }}
                    >
                      {row.secondary === "Remove" && removing === name
                        ? "Remove it"
                        : row.secondary}
                    </button>
                  )}
                </span>
              </div>
              {open ? (
                <div class="settings-key-field">
                  <KeyField
                    id={`settings-key-${name}`}
                    label={keyLabel(name)}
                    hint={keyHintFor(keyState)}
                    note={KEY_NOTE}
                    focus={true}
                    disabled={busy}
                    onSave={(value) => {
                      onOpenKey(null);
                      void controller.replaceKey(name, value);
                    }}
                  />
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>
      <Problem text={state.problems.keys} />
    </section>
  );
}

export function CodingSection({ state, status, controller }: SectionProps) {
  const busy = state.busy.has("coding");
  const harnesses = status.harnesses;
  return (
    <section
      id="settings-coding"
      class="settings-section"
      aria-labelledby="settings-coding-title"
      aria-busy={busy}
    >
      <SectionHead id="settings-coding-title" step="CODING" title="Repos and harness" />
      <div class="settings-facts-rows">
        <div class="settings-fact">
          <span>Claude Code</span>
          <span class="settings-mono-quiet">
            {harnessLine(harnesses.claude_code, harnesses.default === "ClaudeCode")}
          </span>
        </div>
        <div class="settings-fact">
          <span>pi</span>
          <span class="settings-mono-quiet">
            {harnessLine(harnesses.pi, harnesses.default === "Pi")}
          </span>
        </div>
        <div class="setup-rule" />
        <div class="settings-fact">
          <span>Code folders</span>
          <button
            type="button"
            class="settings-tiny"
            disabled={busy}
            onClick={() => void controller.addCodeFolder()}
          >
            {busy ? "Choosing…" : "Add a folder"}
          </button>
        </div>
        {status.code_folders.length === 0 ? (
          <span class="setup-note">{NO_CODE_FOLDERS}</span>
        ) : (
          <ul class="settings-folders">
            {status.code_folders.map((folder, index) => (
              <li key={folder} class="settings-fact">
                <span class="settings-mono-quiet">{folder}</span>
                <button
                  type="button"
                  class="settings-quiet-button"
                  disabled={busy}
                  aria-label={`Remove ${folder}`}
                  onClick={() => void controller.removeCodeFolder(index)}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
      <Problem text={state.problems.coding} />
    </section>
  );
}

export function MemorySection({ state, status, controller }: SectionProps) {
  const rebuilding = state.busy.has("memory");
  return (
    <section
      id="settings-memory"
      class="settings-section"
      aria-labelledby="settings-memory-title"
      aria-busy={rebuilding}
    >
      <SectionHead id="settings-memory-title" step="MEMORY AND PRIVACY" title="What it keeps" />
      <div class="settings-facts-rows">
        <div class="settings-fact">
          <span>Your notes, sessions and memories</span>
          <span class="settings-mono-quiet">{status.vault}</span>
        </div>
        <div class="settings-fact">
          <span>Your voice</span>
          <span class="settings-quiet">never stored</span>
        </div>
        <div class="settings-fact">
          <span>Inferred memories</span>
          <span class="settings-quiet">asked first</span>
        </div>
        <div class="setup-rule" />
        <div class="settings-buttons">
          <button type="button" class="settings-tiny" onClick={() => void controller.openVault()}>
            Open the vault
          </button>
          <button
            type="button"
            class="settings-tiny"
            disabled={rebuilding}
            onClick={() => void controller.rebuildIndex()}
          >
            {rebuilding ? "Rebuilding…" : "Rebuild the index"}
          </button>
        </div>
      </div>
      {state.rebuilt === null ? null : <p class="setup-note">{rebuiltLine(state.rebuilt)}</p>}
      <Problem text={state.problems.memory} />
    </section>
  );
}
