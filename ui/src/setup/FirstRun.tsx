// The first-run screen (artboard 11 · First run, LLD §3.8): the one screen that needs typing.
// It shows until "Start talking" starts the session; then the stage takes the window. Over a
// running session the same entry point shows the settings page instead (artboard 12,
// SettingsPage.tsx): the app's status says which, through `complete`.

import { effect } from "@preact/signals";
import { render } from "preact";
import { useEffect, useLayoutEffect, useRef } from "preact/hooks";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import { MicrophoneCard, Problem, ThinkingCard, VoiceCard } from "./cards.tsx";
import { ModelCatalog } from "./catalog.ts";
import { SetupController } from "./controller.ts";
import { mountSetupOrb } from "./orb.ts";
import { SettingsPage } from "./SettingsPage.tsx";
import { canStart } from "./state.ts";
import type { SetupTransport } from "./transport.ts";
import { FOOTER_FIRST_RUN, harnessChip, START_TALKING, startHint } from "./words.ts";

function Harnesses({ status }: { status: SetupStatus }) {
  const chips = [
    { name: "claude", found: status.harnesses.claude_code },
    { name: "pi", found: status.harnesses.pi },
  ];
  return (
    <div class="first-run-harness">
      <span>Coding harness</span>
      {chips.map((chip) => (
        <span
          key={chip.name}
          class={chip.found.state === "Found" ? "setup-chip setup-chip-found" : "setup-chip"}
        >
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

  useLayoutEffect(() => {
    const host = orbHost.current;
    return host === null ? undefined : mountSetupOrb(host, orbSize, still);
  }, [orbSize, still]);

  useEffect(() => {
    // Coming back from System Settings (the microphone) or from starting Ollama.
    const again = (): void => void controller.refresh();
    window.addEventListener("focus", again);
    return () => window.removeEventListener("focus", again);
  }, [controller]);

  const status = state.status;
  const hint = status === null || state.busy.size > 0 ? null : startHint(status);
  return (
    <main class="first-run" aria-label="Set up BoardAI">
      <div class="first-run-column">
        <div class="first-run-orb" ref={orbHost} />
        <header class="first-run-head">
          <h1 class="first-run-title">Before we talk</h1>
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
              <span class="first-run-later">{FOOTER_FIRST_RUN}</span>
            </div>
            <div class="first-run-start">
              <button
                type="button"
                class="first-run-go"
                disabled={!canStart(state)}
                aria-describedby={hint === null ? undefined : "first-run-hint"}
                onClick={() => void controller.start()}
              >
                {START_TALKING}
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
 * The first run, or the settings page over a running session (the status says which). Nothing
 * shows until the status is known, so settings never flash the first run's layout.
 */
function SetupScreen({
  controller,
  catalog,
  view,
}: {
  controller: SetupController;
  catalog: ModelCatalog;
  view: FirstRunView;
}) {
  const state = controller.state.value;
  useEffect(() => {
    void controller.load();
  }, [controller]);
  if (state.status?.complete === true) {
    return <SettingsPage controller={controller} catalog={catalog} still={view.still} />;
  }
  if (state.status === null && state.loadProblem === null) {
    return null;
  }
  return <FirstRun controller={controller} orbSize={view.orbSize} still={view.still} />;
}

/**
 * Shows the first-run screen in `container` (or the settings page, over a running session);
 * `onDone` runs once the session has started or settings close (the caller renders the stage
 * there, or removes the page).
 */
export function showFirstRun(
  container: HTMLElement,
  transport: SetupTransport,
  status: SetupStatus | null,
  view: FirstRunView,
  onDone: () => void,
): void {
  const controller = new SetupController(transport, status);
  const catalog = new ModelCatalog(transport);
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
  render(<SetupScreen controller={controller} catalog={catalog} view={view} />, container);
}
