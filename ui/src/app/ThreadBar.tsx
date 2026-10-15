// The thread (PRODUCT §6.3): a thin line of step dots at top center, the current step pulsing in
// signal pink with its name under it, and each open detour as a small pink branch.

import { useMemo, useRef } from "preact/hooks";
import { easeOut, ramp, round1, round2 } from "../canvas/math.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";

/** Branch geometry from the design, relative to the current dot: down, then out to the label. */
const BRANCH_DROP = 40;
const BRANCH_REACH = 58;
const BRANCH_LABEL_X = 66;
const BRANCH_STACK = 24;

function branchPath(x: number, y: number, depth: number): string {
  const drop = BRANCH_DROP + depth * BRANCH_STACK;
  return `M${x} ${y}Q${x} ${y + drop} ${x + 38} ${y + drop}L${x + BRANCH_REACH} ${y + drop}`;
}

export function ThreadBar() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const thread = runtime.scene.store.thread.value;
  const rootRef = useRef<HTMLElement>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const haloRef = useRef<HTMLDivElement>(null);
  const labelRef = useRef<HTMLDivElement>(null);
  const branchRefs = useRef<(SVGPathElement | null)[]>([]);
  const branchLabelRefs = useRef<(HTMLDivElement | null)[]>([]);
  const write = useMemo(styleWriter, []);
  const gap = tokens.threadStepGap;

  useFrame((at) => {
    if (thread === null) {
      return;
    }
    const shown = easeOut(ramp(at, thread.shownAt, thread.shownAt + tokens.durationThreadInMs));
    write(rootRef.current, "opacity", String(round2(shown)));
    write(rootRef.current, "transform", `translateY(${round1((1 - shown) * 12)}px)`);
    write(rootRef.current, "filter", `blur(${round1((1 - shown) * 8)}px)`);
    let progress = 0;
    for (let index = 1; index < thread.reachedAt.length; index += 1) {
      const reached = thread.reachedAt[index];
      if (reached !== null && reached !== undefined) {
        progress += easeOut(ramp(at, reached, reached + tokens.durationStandardMs));
      }
    }
    write(progressRef.current, "width", `${round1(gap * progress)}px`);
    const detourOpen = thread.detours.some(
      (detour) => detour.closedAt === null || at < detour.closedAt + 300,
    );
    write(haloRef.current, "opacity", detourOpen ? "0" : "1");
    write(
      haloRef.current,
      "box-shadow",
      `0 0 0 ${round1(4 + 2 * Math.sin((at / 1000) * 3))}px rgba(var(--signal-rgb), 0.14)`,
    );
    const labelIn = easeOut(
      ramp(at, thread.currentAt, thread.currentAt + tokens.durationStandardMs),
    );
    const detourFade = thread.detours.reduce((most, detour) => {
      const open = ramp(at, detour.openAt, detour.openAt + 300);
      const close = detour.closedAt === null ? 0 : ramp(at, detour.closedAt, detour.closedAt + 300);
      return Math.max(most, open * (1 - close));
    }, 0);
    write(labelRef.current, "opacity", String(round2(labelIn * (1 - detourFade))));
    write(labelRef.current, "transform", `translate(-50%, ${round1((1 - labelIn) * 4)}px)`);
    thread.detours.forEach((detour, index) => {
      const grow = easeOut(ramp(at, detour.openAt, detour.openAt + tokens.durationBranchInMs));
      const shrink =
        detour.closedAt === null
          ? 0
          : easeOut(ramp(at, detour.closedAt, detour.closedAt + tokens.durationBranchInMs - 50));
      const reveal = grow * (1 - shrink);
      const path = branchRefs.current[index] ?? null;
      if (path !== null) {
        const length = path.getTotalLength();
        write(path, "stroke-dasharray", `${round1(length)} ${round1(length + 20)}`);
        write(path, "stroke-dashoffset", String(round1(length * (1 - reveal))));
      }
      const label = branchLabelRefs.current[index] ?? null;
      write(label, "opacity", String(round2(ramp(reveal, 0.6, 1))));
      write(label, "transform", `translateX(${round1((1 - reveal) * -6)}px)`);
    });
  });

  if (thread === null || thread.steps.length === 0) {
    return null;
  }
  const count = thread.steps.length;
  const width = gap * (count - 1);
  const current = thread.current;
  const detourOpen = thread.detours.some((detour) => detour.closedAt === null);
  const anchor = current ?? 0;
  return (
    <section class="thread" ref={rootRef} aria-label="Explanation steps">
      <div class="thread-track" style={{ width: `${width}px`, marginLeft: `${-width / 2}px` }} />
      <div class="thread-progress" ref={progressRef} style={{ marginLeft: `${-width / 2}px` }} />
      <svg class="thread-branches" aria-hidden="true" width="1" height="1">
        {thread.detours.map((detour, index) => (
          <path
            key={`${detour.title}-${detour.openAt}`}
            ref={(element) => {
              branchRefs.current[index] = element;
            }}
            d={branchPath(-width / 2 + gap * anchor, 0, index)}
          />
        ))}
      </svg>
      {thread.steps.map((step, index) => {
        const state =
          current === null || index > current
            ? "ahead"
            : index < current
              ? "passed"
              : detourOpen
                ? "paused"
                : "current";
        return (
          <div
            key={`${index}-${step}`}
            class={`thread-dot thread-dot-${state}`}
            style={{ left: `${-width / 2 + gap * index}px` }}
            aria-current={index === current ? "step" : undefined}
            title={step}
          />
        );
      })}
      {current !== null ? (
        <div
          class="thread-halo"
          ref={haloRef}
          aria-hidden="true"
          style={{ left: `${-width / 2 + gap * current}px` }}
        />
      ) : null}
      {current !== null ? (
        <div
          class="thread-label"
          ref={labelRef}
          style={{ left: `${-width / 2 + gap * current}px` }}
        >
          {thread.steps[current]}
        </div>
      ) : null}
      {thread.detours.map((detour, index) => (
        <div
          key={`${detour.title}-${detour.openAt}`}
          class="thread-detour"
          ref={(element) => {
            branchLabelRefs.current[index] = element;
          }}
          style={{
            left: `${-width / 2 + gap * anchor + BRANCH_LABEL_X}px`,
            top: `${tokens.threadTop + BRANCH_DROP - 8 + index * BRANCH_STACK}px`,
          }}
        >
          <span class="thread-detour-dot" />
          <span>{detour.title}</span>
        </div>
      ))}
    </section>
  );
}
