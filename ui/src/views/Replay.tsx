// A past session played back like a film (PRODUCT §7.6), over the live stage: its board, thread,
// surfaces and captions, from its events.jsonl, with the replay controls. It runs on its own
// runtime and replay transport, so the live session underneath is untouched; the live orb and
// captions stay on top. Opened by voice (a Replay view) or from a card's play button.

import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { Captions } from "../app/Captions.tsx";
import { Chips } from "../app/Chips.tsx";
import { Column } from "../app/Column.tsx";
import { useRuntime } from "../app/hooks.ts";
import { ReplayControls } from "../app/ReplayControls.tsx";
import { Runtime, RuntimeContext } from "../app/runtime.ts";
import { ThreadBar } from "../app/ThreadBar.tsx";
import type { SessionCard } from "../protocol/generated/SessionCard.ts";
import { parseReplayLog } from "../transport/log.ts";
import { ReplayTransport } from "../transport/replay.ts";
import { replaySubtitle } from "./labels.ts";
import { CloseGlyph } from "./parts.tsx";

type Loaded =
  | { readonly kind: "loading" }
  | { readonly kind: "failed" }
  | { readonly kind: "ready"; readonly runtime: Runtime; readonly replay: ReplayTransport };

function ReplayStage({
  runtime,
  replay,
  card,
}: {
  runtime: Runtime;
  replay: ReplayTransport;
  card: SessionCard;
}) {
  const outer = useRuntime();
  const stageRef = useRef<HTMLDivElement>(null);
  const boardRef = useRef<HTMLDivElement>(null);
  const orbRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const stage = stageRef.current;
    const board = boardRef.current;
    const orb = orbRef.current;
    if (stage === null || board === null || orb === null) {
      return;
    }
    const unmount = runtime.mount(stage, board, orb);
    replay.play(performance.now());
    const durationMs = replay.log.durationMs;
    const stop = runtime.onFrame((at) => outer.replays.record(card.id, at / durationMs));
    return () => {
      stop();
      unmount();
    };
  }, [runtime, replay, outer, card.id]);
  return (
    <RuntimeContext.Provider value={runtime}>
      <div class="replay-stage" ref={stageRef}>
        <div class="board" ref={boardRef} />
        {/* The live orb is BoardAI's presence; the replayed one is not shown. */}
        <div class="orb-host replay-orb" ref={orbRef} aria-hidden="true" />
        <ThreadBar />
        <Column />
        <Chips />
        <Captions />
      </div>
    </RuntimeContext.Provider>
  );
}

/**
 * The overlay's controls (7b): the title and the replay line stacked above one compact row at
 * bottom left (play, the chapter track, time, chapter, close) that ends well clear of the orb at
 * bottom center.
 */
function CloseButton({ onClose }: { onClose: () => void }) {
  return (
    <button
      type="button"
      class="card-control replay-close"
      aria-label="Close the replay"
      onClick={onClose}
    >
      <CloseGlyph />
    </button>
  );
}

export function ReplayOverlay({
  card,
  today,
  onClose,
}: {
  card: SessionCard;
  /** The day the view was read, for "Oct 12" ("" when unknown: the day reads in full). */
  today: string;
  onClose: () => void;
}) {
  const outer = useRuntime();
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  useEffect(() => {
    let current = true;
    setLoaded({ kind: "loading" });
    void outer.transport.sessionLog(card.id).then((text) => {
      if (!current) {
        return;
      }
      const log = text === null ? null : parseReplayLog(text);
      if (log === null || log.batches.length === 0) {
        setLoaded({ kind: "failed" });
        return;
      }
      const replay = new ReplayTransport(log);
      const runtime = new Runtime(replay, replay, outer.tokens, outer.reducedMotion);
      setLoaded({ kind: "ready", runtime, replay });
    });
    return () => {
      current = false;
    };
  }, [outer, card.id]);

  return (
    <section class="replay-overlay" aria-label={`Replay of ${card.title}`}>
      {loaded.kind === "ready" ? (
        <ReplayStage runtime={loaded.runtime} replay={loaded.replay} card={card} />
      ) : null}
      <div class="replay-bar">
        <div class="replay-bar-head">
          <span class="replay-bar-title">{card.title}</span>
          <span class="replay-bar-sub">{replaySubtitle(card, today)}</span>
        </div>
        {loaded.kind === "ready" ? (
          <RuntimeContext.Provider value={loaded.runtime}>
            <ReplayControls
              replay={loaded.replay}
              keyboard={false}
              trailing={<CloseButton onClose={onClose} />}
            />
          </RuntimeContext.Provider>
        ) : (
          <div class="replay">
            <CloseButton onClose={onClose} />
          </div>
        )}
      </div>
      {loaded.kind === "failed" ? (
        <p class="replay-problem">I couldn't open that session.</p>
      ) : null}
      {loaded.kind === "loading" ? <p class="replay-loading">Opening the session…</p> : null}
    </section>
  );
}
