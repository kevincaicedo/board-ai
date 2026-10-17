// Drawing together on the stage (13 · You draw): the tools at bottom left, "You're drawing ·
// BoardAI is watching" at the top, the hint under the shape your words will label, and, while you
// draw, a surface over the board that turns the pointer into shapes. What is committed comes back
// from the core (UserShape) and the canvas draws it; until then the pointer's preview stays.

import type { TargetedPointerEvent } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { styleWriter, useFrame, useRuntime } from "../app/hooks.ts";
import type { DrawAction } from "../protocol/generated/DrawAction.ts";
import { hintPlacement, hintWords } from "./hint.ts";
import { shapeBounds } from "./ink.ts";
import { drawnShape, previewPath, StrokeSampler } from "./input.ts";
import { TOOLS, type Tool } from "./model.ts";

/** Previews waiting for the core's echo; past this the oldest goes at once. */
export const PENDING_PREVIEWS_MAX = 8;

const TOOL_LABELS: { readonly [K in Tool]: string } = {
  pen: "Pen",
  box: "Box",
  arrow: "Arrow",
};

function ToolGlyph({ tool, pressed }: { tool: Tool; pressed: boolean }) {
  const stroke = pressed ? "var(--ink-you)" : "var(--ink)";
  switch (tool) {
    case "pen":
      return (
        <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" stroke={stroke}>
          <path d="M4 20l4-1 11-11-3-3L5 16z" />
        </svg>
      );
    case "box":
      return (
        <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" stroke={stroke}>
          <rect x="4" y="6" width="16" height="12" rx="1.5" />
        </svg>
      );
    case "arrow":
      return (
        <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" stroke={stroke}>
          <path d="M4 12h15M14 7l5 5-5 5" />
        </svg>
      );
  }
}

function MicGlyph({ size }: { size: number }) {
  return (
    <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" class="draw-mic">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5.5 11a6.5 6.5 0 0 0 13 0" />
    </svg>
  );
}

function Tools({ raised }: { raised: boolean }) {
  const runtime = useRuntime();
  const draw = runtime.scene.draw;
  const drawing = draw.drawing.value;
  const tool = draw.tool.value;
  const muted = runtime.scene.store.orb.value.state === "Muted";
  const send = (action: DrawAction): void => runtime.send({ type: "Draw", action });
  const className = raised ? "draw-tools draw-tools-raised" : "draw-tools";
  if (!drawing) {
    // A replay shows what was drawn; there is no drawing to start in one.
    if (runtime.replay !== null) {
      return null;
    }
    return (
      <div class={className}>
        <button
          type="button"
          class="draw-tool"
          aria-label="Draw with BoardAI (D)"
          onClick={() => send({ kind: "Start" })}
        >
          <ToolGlyph tool="pen" pressed={false} />
        </button>
      </div>
    );
  }
  return (
    <div class={className} role="toolbar" aria-label="Drawing tools">
      {TOOLS.map((each) => (
        <button
          key={each}
          type="button"
          class={each === tool ? "draw-tool draw-tool-on" : "draw-tool"}
          aria-label={TOOL_LABELS[each]}
          aria-pressed={each === tool}
          onClick={() => {
            draw.tool.value = each;
          }}
        >
          <ToolGlyph tool={each} pressed={each === tool} />
        </button>
      ))}
      <button
        type="button"
        class="draw-tool"
        aria-label="Undo the last stroke"
        onClick={() => send({ kind: "Undo" })}
      >
        <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" stroke="var(--ink)">
          <path d="M9 14L4 9l5-5" />
          <path d="M4 9h10a6 6 0 0 1 0 12h-3" />
        </svg>
      </button>
      <span class="draw-tools-rule" aria-hidden="true" />
      <button
        type="button"
        class="draw-done"
        disabled={muted}
        title={muted ? "Unmute to have BoardAI answer" : undefined}
        onClick={() => send({ kind: "Done" })}
      >
        Done
      </button>
    </div>
  );
}

function Watching() {
  const runtime = useRuntime();
  if (!runtime.scene.draw.drawing.value) {
    return null;
  }
  return (
    <div class="draw-watching" role="status">
      <span class="draw-watching-dot" aria-hidden="true" />
      You're drawing · BoardAI is watching
    </div>
  );
}

function Hint() {
  const runtime = useRuntime();
  const draw = runtime.scene.draw;
  const hint = draw.hint.value;
  const pillRef = useRef<HTMLDivElement>(null);
  const write = useMemo(styleWriter, []);
  // The camera and the target as last placed: the pill moves only when one of them did, so a
  // still frame allocates nothing.
  const placed = useMemo(
    () => ({ target: null as unknown, pill: null as HTMLElement | null, at: new Float64Array(3) }),
    [],
  );
  useFrame(() => {
    const target = hint === null ? null : draw.shape(0, hint.target);
    const pill = pillRef.current;
    const board = runtime.board;
    if (target === null || pill === null || board === null) {
      return;
    }
    const camera = board.mainTransform;
    const last = placed.at;
    if (
      placed.target === target &&
      placed.pill === pill &&
      last[0] === camera.scale &&
      last[1] === camera.x &&
      last[2] === camera.y
    ) {
      return;
    }
    placed.target = target;
    placed.pill = pill;
    last[0] = camera.scale;
    last[1] = camera.x;
    last[2] = camera.y;
    const place = hintPlacement(shapeBounds(target.shape), camera, runtime.tokens);
    write(pill, "transform", `translate(${place.left}px, ${place.top}px)`);
  });
  if (hint === null || draw.shape(0, hint.target) === null) {
    return null;
  }
  return (
    <div class="draw-hint" ref={pillRef} aria-hidden="true">
      <MicGlyph size={14} />
      <span>{hintWords(hint.words)}</span>
      <span class="draw-hint-action">→ will label this shape</span>
    </div>
  );
}

interface Pending {
  readonly serial: number;
  readonly d: string;
  readonly echoes: number;
  readonly sentAt: number;
}

/**
 * The pointer surface over the board while you draw: preview while down, a shape when up. It is
 * mounted right over the board, under every control, so the mic, the cards and the chips stay
 * usable while you draw.
 */
export function DrawingSurface() {
  const runtime = useRuntime();
  const draw = runtime.scene.draw;
  const sampler = useMemo(() => new StrokeSampler(), []);
  const liveRef = useRef<SVGPathElement>(null);
  const active = useRef<{ pointer: number; tool: Tool } | null>(null);
  const serial = useRef(0);
  const [pending, setPending] = useState<readonly Pending[]>([]);
  const echoes = draw.echoes.value;
  const holdMs = runtime.tokens.durationDrawPreviewHoldMs;

  // An echo (or the hold running out: the core refused it) takes the oldest preview away.
  useEffect(() => {
    setPending((previews) => previews.filter((preview) => preview.echoes > echoes));
  }, [echoes]);
  useEffect(() => {
    if (pending.length === 0) {
      return;
    }
    const oldest = pending[0];
    const wait = oldest === undefined ? 0 : Math.max(0, oldest.sentAt + holdMs - performance.now());
    const timer = setTimeout(() => {
      const now = performance.now();
      setPending((previews) => previews.filter((preview) => now - preview.sentAt < holdMs));
    }, wait);
    return () => clearTimeout(timer);
  }, [pending, holdMs]);

  if (!draw.drawing.value || runtime.replay !== null) {
    return null;
  }
  const local = (event: PointerEvent): [number, number] => {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  };
  const showLive = (): void => {
    const tool = active.current?.tool ?? draw.tool.peek();
    liveRef.current?.setAttribute("d", previewPath(tool, sampler));
  };
  const down = (event: TargetedPointerEvent<HTMLDivElement>): void => {
    if (event.button !== 0 || active.current !== null) {
      return;
    }
    event.currentTarget.setPointerCapture(event.pointerId);
    active.current = { pointer: event.pointerId, tool: draw.tool.peek() };
    const [x, y] = local(event);
    sampler.begin(x, y);
    showLive();
  };
  const move = (event: TargetedPointerEvent<HTMLDivElement>): void => {
    if (active.current?.pointer !== event.pointerId) {
      return;
    }
    const [x, y] = local(event);
    if (sampler.add(x, y)) {
      showLive();
    }
  };
  const up = (event: TargetedPointerEvent<HTMLDivElement>): void => {
    const gesture = active.current;
    if (gesture === null || gesture.pointer !== event.pointerId) {
      return;
    }
    active.current = null;
    const [x, y] = local(event);
    sampler.finish(x, y);
    const d = previewPath(gesture.tool, sampler);
    liveRef.current?.setAttribute("d", "");
    const board = runtime.board;
    const shape = board === null ? null : drawnShape(gesture.tool, sampler, board.mainTransform);
    if (shape === null) {
      return;
    }
    runtime.send({ type: "Draw", action: { kind: "Shape", layer: 0, drawn: shape } });
    serial.current += 1;
    const preview = {
      serial: serial.current,
      d,
      echoes: draw.echoes.peek() + 1,
      sentAt: performance.now(),
    };
    setPending((previews) => [...previews, preview].slice(-PENDING_PREVIEWS_MAX));
  };
  const cancel = (event: TargetedPointerEvent<HTMLDivElement>): void => {
    if (active.current?.pointer === event.pointerId) {
      active.current = null;
      liveRef.current?.setAttribute("d", "");
    }
  };
  return (
    <div
      class="draw-surface"
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={cancel}
    >
      <svg class="draw-preview" aria-hidden="true">
        {pending.map((preview) => (
          <path key={preview.serial} d={preview.d} />
        ))}
        <path ref={liveRef} d="" />
      </svg>
    </div>
  );
}

/**
 * Drawing together's controls: the line, the hint and the tools (the surface mounts over the
 * board). `raised` clears a replay's controls.
 */
export function Drawing({ raised }: { raised: boolean }) {
  return (
    <>
      <Watching />
      <Hint />
      <Tools raised={raised} />
    </>
  );
}
