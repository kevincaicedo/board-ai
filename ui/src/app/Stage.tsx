// The full-bleed stage (PRODUCT §8.7): no chrome, the canvas fills the window, the orb and the
// surfaces float over it. The canvas and orb are imperative views mounted into hosts here.

import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { Views } from "../views/Views.tsx";
import { Captions } from "./Captions.tsx";
import { Chips } from "./Chips.tsx";
import { Column } from "./Column.tsx";
import { MicButton } from "./MicButton.tsx";
import { ReplayControls } from "./ReplayControls.tsx";
import { type Runtime, RuntimeContext } from "./runtime.ts";
import { Suggestion } from "./Suggestion.tsx";
import { ThreadBar } from "./ThreadBar.tsx";

/** How often the optional frame-cost readout refreshes. */
const PERF_REFRESH_MS = 1000;

function PerfReadout({ runtime }: { runtime: Runtime }) {
  const [text, setText] = useState("");
  useEffect(() => {
    const timer = setInterval(() => {
      const { frames, worstMs, meanMs, worstEverMs } = runtime.stats;
      setText(
        `frames ${frames} · last ${worstMs.toFixed(2)} ms worst, ${meanMs.toFixed(2)} ms mean · ever ${worstEverMs.toFixed(2)} ms`,
      );
    }, PERF_REFRESH_MS);
    return () => clearInterval(timer);
  }, [runtime]);
  return (
    <output class="perf" id="perf">
      {text}
    </output>
  );
}

export function Stage({
  runtime,
  perf,
  controls,
}: {
  runtime: Runtime;
  perf: boolean;
  controls: boolean;
}) {
  const stageRef = useRef<HTMLElement>(null);
  const boardRef = useRef<HTMLDivElement>(null);
  const orbRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const stage = stageRef.current;
    const board = boardRef.current;
    const orb = orbRef.current;
    if (stage === null || board === null || orb === null) {
      return;
    }
    return runtime.mount(stage, board, orb);
  }, [runtime]);

  return (
    <RuntimeContext.Provider value={runtime}>
      <main class="stage" ref={stageRef} aria-label="BoardAI">
        {/* The shell lets a view soften the board without touching the board's own styles. */}
        <div class="board-shell">
          <div class="board" ref={boardRef} />
        </div>
        <div class="orb-host" ref={orbRef} />
        <Suggestion />
        <ThreadBar />
        <Column />
        <Views />
        <Chips />
        <Captions />
        <MicButton />
        {runtime.replay !== null && controls ? <ReplayControls replay={runtime.replay} /> : null}
        {perf ? <PerfReadout runtime={runtime} /> : null}
      </main>
    </RuntimeContext.Provider>
  );
}
