// The first-run screen (artboard 11 · First run, LLD §3.8): the one screen that needs typing.
// It shows until "Start talking" starts the session; then the stage takes the window. Over a
// running session it is the settings screen (settings.tsx).

import { effect } from "@preact/signals";
import { render } from "preact";
import { useEffect, useLayoutEffect, useRef } from "preact/hooks";
import { opensSettings } from "../app/keys.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { MicrophoneCard, Problem, ThinkingCard, VoiceCard } from "./cards.tsx";
import { SetupController } from "./controller.ts";
import { mountSetupOrb } from "./orb.ts";
import { canStart } from "./state.ts";
import type { SetupTransport } from "./transport.ts";
import { harnessChip, startHint } from "./words.ts";

function Harnesses({ status }: { status: SetupStatus }) {
  const chips = [
    { name: "claude", found: status.harnesses.claude_code },
    { name: "pi", found: status.harnesses.pi },
  ];
  return (
    <div class="first-run-harness">
      <span>Coding harness</span>
      {chips.map((chip) => (
        <span key={chip.name} class={chip.found ? "setup-chip setup-chip-found" : "setup-chip"}>
          {harnessChip(chip.name, chip.found)}
        </span>
      ))}
    </div>
  );
}

/** How the screen draws its orb. */
export interface FirstRunView {
  readonly orbSize: number;
  readonly still: boolean;
}

export function FirstRun({
  controller,
  orbSize,
  still,
}: {
  controller: SetupController;
  orbSize: number;
  still: boolean;
}) {
  const state = controller.state.value;
  const orbHost = useRef<HTMLDivElement>(null);
  const screen = useRef<HTMLElement>(null);
  // Opened over a running session (⌘, or "open settings"): the same screen, as settings.
  const settings = state.status?.complete === true;

  useEffect(() => {
    if (settings) {
      screen.current?.focus();
    }
  }, [settings]);

  useLayoutEffect(() => {
    const host = orbHost.current;
    return host === null ? undefined : mountSetupOrb(host, orbSize, still);
  }, [orbSize, still]);

  useEffect(() => {
    void controller.load();
    // Coming back from System Settings (the microphone) or from starting Ollama.
    const again = (): void => void controller.refresh();
    window.addEventListener("focus", again);
    return () => window.removeEventListener("focus", again);
  }, [controller]);

  const status = state.status;
  const hint = status === null || state.busy.size > 0 ? null : startHint(status);
  return (
    <main
      class="first-run"
      aria-label={settings ? "Settings" : "Set up BoardAI"}
      ref={screen}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (!settings) {
          return;
        }
        // The stage's keys (Space, M, Esc) wait while settings are open.
        event.stopPropagation();
        const shortcut = opensSettings({
          code: event.code,
          meta: event.metaKey,
          repeat: event.repeat,
        });
        if (event.code === "Escape" || shortcut) {
          event.preventDefault();
          void controller.leave();
        }
      }}
    >
      <div class="first-run-column">
        <div class="first-run-orb" ref={orbHost} />
        <header class="first-run-head">
          <h1 class="first-run-title">{settings ? "Settings" : "Before we talk"}</h1>
          <p class="first-run-lede">
            This is the only screen you type on. Keys go to your macOS Keychain; everything else
            stays in ~/BoardAI.
          </p>
        </header>
        {status === null ? (
          <Problem text={state.loadProblem} />
        ) : (
          <>
            <div class="first-run-cards">
              <ThinkingCard state={state} status={status} controller={controller} />
              <VoiceCard state={state} status={status} controller={controller} />
              <MicrophoneCard state={state} status={status} controller={controller} />
            </div>
            <div class="first-run-foot">
              <Harnesses status={status} />
              <span class="first-run-later">
                {settings
                  ? "Esc closes settings. A change starts a new session."
                  : "Change any of this later by saying “open settings”."}
              </span>
            </div>
            <div class="first-run-start">
              <button
                type="button"
                class="first-run-go"
                disabled={!canStart(state)}
                aria-describedby={hint === null ? undefined : "first-run-hint"}
                onClick={() => void controller.start()}
              >
                Start talking
              </button>
              {hint === null ? null : (
                <p id="first-run-hint" class="setup-note">
                  {hint}
                </p>
              )}
              <Problem text={state.problems.start} />
            </div>
          </>
        )}
      </div>
    </main>
  );
}

/**
 * Shows the first-run screen in `container`; `onDone` runs once the session has started (the
 * caller renders the stage there).
 */
export function showFirstRun(
  container: HTMLElement,
  transport: SetupTransport,
  status: SetupStatus | null,
  view: FirstRunView,
  onDone: () => void,
): void {
  const controller = new SetupController(transport, status);
  let done = false;
  const stop = effect(() => {
    if (controller.state.value.finished && !done) {
      done = true;
      queueMicrotask(() => {
        stop();
        onDone();
      });
    }
  });
  render(<FirstRun controller={controller} orbSize={view.orbSize} still={view.still} />, container);
}
