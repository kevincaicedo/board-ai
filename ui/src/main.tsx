import { render } from "preact";
import { Runtime } from "./app/runtime.ts";
import { Stage } from "./app/Stage.tsx";
import "./styles/tokens.css";
import "./styles/stage.css";
import "./styles/canvas.css";
import "./styles/views.css";
import "./styles/panels.css";
import "./styles/setup.css";
import "./styles/draw.css";
import { showFirstRun } from "./setup/FirstRun.tsx";
import { isPreset, presetTransport } from "./setup/fake.ts";
import { installSettings } from "./setup/settings.tsx";
import { TauriSetupTransport } from "./setup/tauriSetup.ts";
import { readTokens } from "./styles/tokens.ts";
import { parseReplayLog, REPLAY_TAIL_MS, type ReplayLog } from "./transport/log.ts";
import { ReplayTransport } from "./transport/replay.ts";
import { chooseTransport } from "./transport/select.ts";
import { insideTauri, TauriTransport, tauriBridge } from "./transport/tauri.ts";

const root = document.getElementById("app");
if (root === null) {
  throw new Error("index.html must contain #app");
}

/** A replay that couldn't load still opens, and says so where captions go. */
function failedReplay(url: string): ReplayLog {
  return {
    batches: [
      { at: 0, events: [{ type: "Problem", spoken: "I couldn't open that replay.", detail: url }] },
    ],
    durationMs: REPLAY_TAIL_MS,
    chapters: [{ at: 0, kind: "Idle" }],
    skipped: { malformed: 0, otherKind: 0, overLimit: 0, rows: 0 },
  };
}

async function loadReplay(url: string): Promise<ReplayLog> {
  try {
    const response = await fetch(url);
    if (!response.ok) {
      return failedReplay(url);
    }
    const log = parseReplayLog(await response.text());
    const { malformed, otherKind, overLimit, rows } = log.skipped;
    if (malformed + overLimit + rows > 0) {
      console.warn(
        `replay ${url}: skipped ${malformed} malformed and ${overLimit} over-limit lines, and ${rows} unreadable view rows`,
      );
    }
    if (otherKind > 0) {
      console.info(`replay ${url}: ${otherKind} non-UI lines (core inputs) ignored`);
    }
    return log;
  } catch {
    return failedReplay(url);
  }
}

/** Handwriting must not start in a fallback face: load the three faces before playing. */
async function loadFonts(): Promise<void> {
  try {
    await Promise.all([
      document.fonts.load("20px Excalifont"),
      document.fonts.load("300 18px Geist"),
      document.fonts.load("400 18px Geist"),
      document.fonts.load("500 13px Geist"),
      document.fonts.load('400 12.5px "Geist Mono"'),
    ]);
  } catch {
    // A missing face falls back to the next in the stack; the replay still plays.
  }
}

async function boot(container: HTMLElement): Promise<void> {
  const tokens = readTokens(getComputedStyle(document.documentElement));
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const params = new URLSearchParams(location.search);
  const choice = chooseTransport(location.search, insideTauri(window));
  const firstRunView = { orbSize: tokens.setupOrb, still: reducedMotion };
  const stage = (runtime: Runtime): void => {
    render(
      <Stage
        runtime={runtime}
        perf={params.has("perf")}
        controls={params.get("controls") !== "0"}
      />,
      container,
    );
  };
  if (choice.kind === "tauri") {
    const bridge = tauriBridge();
    const setup = new TauriSetupTransport(bridge);
    const live = (): void => {
      const runtime = new Runtime(
        new TauriTransport(bridge, () => performance.now()),
        null,
        tokens,
        reducedMotion,
      );
      stage(runtime);
      // ⌘, or "open settings" shows the same screen over the stage.
      installSettings(runtime.scene.store, setup, firstRunView);
    };
    // The first run (LLD §3.8): the session waits for "Start talking". A shell without the
    // setup commands opens the stage as before.
    const status = await setup.status().catch(() => null);
    if (status !== null && !status.complete) {
      showFirstRun(container, setup, status, firstRunView, live);
    } else {
      live();
    }
    return;
  }
  const preset = params.get("setup");
  if (preset !== null && isPreset(preset)) {
    // The first-run screen with a fake app, for the visual check against the canvas.
    showFirstRun(container, presetTransport(preset), null, firstRunView, () => {
      location.assign(location.pathname);
    });
    return;
  }
  const replay = new ReplayTransport(await loadReplay(choice.url));
  const runtime = new Runtime(replay, replay, tokens, reducedMotion);
  await loadFonts();
  const now = performance.now();
  replay.setSpeed(choice.speed, now);
  if (choice.seekMs === null) {
    replay.play(now);
  } else {
    replay.seek(choice.seekMs, now);
  }
  stage(runtime);
}

boot(root).catch((error: unknown) => {
  // A boot failure is a build or packaging defect (a missing token, say): say so on screen
  // instead of leaving a blank window.
  root.textContent = `BoardAI couldn't start: ${error instanceof Error ? error.message : String(error)}`;
  console.error(error);
});
