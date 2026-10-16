// The idle suggestion under the orb (frame 1, PRODUCT §8.6: "Pick up Raft log compaction where we
// left off?"). The core sends it at session start and clears it when you first speak; this only
// shows it, with the design's rise-in and blur-out written from the frame loop.

import { useMemo, useRef } from "preact/hooks";
import { round1, round2 } from "../canvas/math.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";
import { suggestionLook } from "./layout.ts";

export function Suggestion() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const view = runtime.scene.store.suggestion.value;
  const lineRef = useRef<HTMLDivElement>(null);
  const write = useMemo(styleWriter, []);

  useFrame((at) => {
    if (view === null) {
      return;
    }
    const look = suggestionLook(view.shownAt, view.clearedAt, at, tokens);
    write(lineRef.current, "opacity", String(round2(look.opacity)));
    write(lineRef.current, "transform", `translateY(${round1(look.rise)}px)`);
    write(lineRef.current, "filter", `blur(${round1(look.blur)}px)`);
  });

  if (view === null) {
    return null;
  }
  return (
    <div class="suggestion" ref={lineRef} aria-live="polite">
      {view.text}
    </div>
  );
}
