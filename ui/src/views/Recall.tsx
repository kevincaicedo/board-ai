// Recall (Session · recall a note and a past session): a past session's board, drawn at once in
// its own layer over the live board, which steps back. Beside it, "From your brain" with the
// session's recap, and the session replay card, whose play button replays it here.

import { useLayoutEffect, useMemo, useRef } from "preact/hooks";
import { styleWriter, useFrame, useRuntime } from "../app/hooks.ts";
import type { ViewEntry } from "../app/views.ts";
import { viewMotion } from "../app/views.ts";
import { cardArea, emptyArea, type Viewport } from "../canvas/camera.ts";
import type { Vec } from "../canvas/geometry.ts";
import { easeOut, ramp, round1, round2 } from "../canvas/math.ts";
import { Presence } from "../canvas/presence.ts";
import { BoardView } from "../canvas/renderer.ts";
import type { View } from "../protocol/generated/View.ts";
import { Cover } from "./Cover.tsx";
import { coverFromOps } from "./cover.ts";
import { capitalized, clockLabel, dayLabel, joined, sessionMeta } from "./labels.ts";
import { MemoryGlyph, PlayGlyph, ProgressTrack } from "./parts.tsx";
import { framePastBoard, pastArea, pastBoardModel } from "./pastBoard.ts";

const THUMB_WIDTH = 168;
const THUMB_HEIGHT = 126;

/** Draws the past board into `host` on the runtime's frame loop until unmounted. */
function usePastBoard(
  host: { readonly current: HTMLDivElement | null },
  entry: ViewEntry,
  view: Extract<View, { kind: "PastSession" }>,
): void {
  const runtime = useRuntime();
  useLayoutEffect(() => {
    const element = host.current;
    if (element === null) {
      return;
    }
    const tokens = runtime.tokens;
    const model = pastBoardModel(view.layers, tokens, entry.shownAt);
    const card: Viewport = { width: tokens.surfaceWidth, height: tokens.detourCardHeight };
    const board = new BoardView(model, new Presence(tokens), element, {
      tokens,
      card,
      reducedMotion: runtime.reducedMotion,
      // A past board is a picture: the core knows nothing of its elements, so pointing at one
      // is not sent.
      onPointer: () => undefined,
    });
    let viewport: Viewport = { width: element.clientWidth, height: element.clientHeight };
    framePastBoard(model, viewport, entry.shownAt);
    const resize = new ResizeObserver(() => {
      viewport = { width: element.clientWidth, height: element.clientHeight };
      framePastBoard(model, viewport, Math.max(entry.shownAt, runtime.at));
    });
    resize.observe(element);
    const anchors: { center: Vec; dock: Vec } = { center: [0, 0], dock: [0, 0] };
    const cardFit = cardArea(card, tokens, emptyArea());
    const stop = runtime.onFrame((at) => board.render(at, pastArea(viewport), cardFit, anchors));
    return () => {
      stop();
      resize.disconnect();
      board.reset();
      element.replaceChildren();
    };
  }, [runtime, host, entry.shownAt, view]);
}

export function RecallView({
  entry,
  view,
}: {
  entry: ViewEntry;
  view: Extract<View, { kind: "PastSession" }>;
}) {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const boardRef = useRef<HTMLDivElement>(null);
  const memoryRef = useRef<HTMLElement>(null);
  const replayRef = useRef<HTMLElement>(null);
  const shellRef = useRef<HTMLElement | null>(null);
  const write = useMemo(styleWriter, []);
  const session = view.session;
  const cover = useMemo(
    () =>
      session.cover.length > 0
        ? session.cover
        : coverFromOps(view.layers.find((layer) => layer.title === null)?.ops ?? []),
    [session.cover, view.layers],
  );
  usePastBoard(boardRef, entry, view);
  useLayoutEffect(() => {
    const shell = runtime.stageElement?.querySelector<HTMLElement>(".board-shell") ?? null;
    shellRef.current = shell;
    return () => {
      shell?.style.removeProperty("filter");
      shell?.style.removeProperty("opacity");
    };
  }, [runtime]);

  useFrame((at) => {
    // The live board steps back while a past one is shown, as under a detour.
    const motion = viewMotion(entry, at, runtime.viewDurations);
    const amount = motion.enter * (1 - motion.exit);
    const shell = shellRef.current;
    write(
      shell,
      "filter",
      amount < 0.01 ? "none" : `blur(${round1(amount * tokens.recallBoardSoften)}px)`,
    );
    write(shell, "opacity", String(round2(1 - 0.5 * amount)));
    // "From your brain" slides in from the brain edge; the replay card rises after it.
    const slide = easeOut(ramp(at, entry.shownAt, entry.shownAt + tokens.durationSceneMs));
    write(
      memoryRef.current,
      "transform",
      `translateX(${round1(tokens.recallCardWidth * (1 - slide))}px)`,
    );
    const rise = easeOut(
      ramp(
        at,
        entry.shownAt + tokens.durationStandardMs,
        entry.shownAt + 2 * tokens.durationStandardMs,
      ),
    );
    write(replayRef.current, "opacity", String(round2(rise)));
    write(
      replayRef.current,
      "transform",
      `translateY(${round1(tokens.surfaceRise * (1 - rise))}px)`,
    );
  });

  const played = runtime.replays.progress.value.get(session.id) ?? 0;
  const day = capitalized(dayLabel(session.date, entry.today));
  return (
    <div class="view-recall">
      <div class="recall-board" ref={boardRef} />
      <div class="recall-cards">
        <section class="recall-card recall-memory" ref={memoryRef} aria-label="From your brain">
          <div class="recall-head">
            <MemoryGlyph tone="signal" />
            <span class="recall-title">From your brain</span>
            <span class="recall-sub">{joined(day, session.title)}</span>
          </div>
          <p class="recall-body">
            {view.recap === "" ? "This session has no digest yet." : view.recap}
          </p>
          <div class="recall-chips">
            <span class="link-chip link-chip-code recall-chip">
              {`session · ${clockLabel(session.duration_ms)}`}
            </span>
          </div>
        </section>
        <section class="recall-card recall-replay" ref={replayRef} aria-label="Session replay">
          <div class="recall-head">
            <span class="recall-head-title">Session replay</span>
            <span class="recall-sub">· jump back in</span>
          </div>
          <div class="recall-replay-body">
            <div class="recall-thumb">
              <Cover shapes={cover} seed={session.id} width={THUMB_WIDTH} height={THUMB_HEIGHT} />
            </div>
            <div class="recall-replay-text">
              <span class="recall-replay-title">{session.title}</span>
              <span class="recall-replay-meta">{sessionMeta(session, entry.today)}</span>
              <div class="recall-replay-controls">
                <button
                  type="button"
                  class="card-control"
                  aria-label={`Play ${session.title}`}
                  onClick={() => runtime.replays.open(session, "recall")}
                >
                  <PlayGlyph />
                </button>
                <ProgressTrack
                  fraction={played}
                  label={`${Math.round(played * 100)}% played`}
                  variant="on-glass"
                />
                <span class="recall-time">
                  {`${clockLabel(played * session.duration_ms)} / ${clockLabel(session.duration_ms)}`}
                </span>
              </div>
            </div>
          </div>
        </section>
      </div>
    </div>
  );
}
