// Captured items (frame 6): a note or task chip rises in where you said it, waits for BoardAI to
// confirm out loud, then flies on an arc into the brain edge, which glows as it lands.

import { useMemo, useRef } from "preact/hooks";
import { bump, easeOut, ramp, round1, round2 } from "../canvas/math.ts";
import type { ChipKind } from "../protocol/generated/ChipKind.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";
import { chipFlight } from "./layout.ts";

function ChipGlyph({ kind }: { kind: ChipKind }) {
  switch (kind) {
    case "Note":
      return (
        <svg class="chip-glyph" aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
          <rect x="2.5" y="2" width="11" height="12" rx="3" />
          <path d="M5.5 6h5M5.5 9h3.5" />
        </svg>
      );
    case "Task":
      return <span class="chip-task-glyph" aria-hidden="true" />;
    case "Memory":
      return (
        <svg class="chip-glyph" aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
          <circle cx="8" cy="8" r="6" />
          <circle class="chip-glyph-fill" cx="8" cy="8" r="2" />
        </svg>
      );
    case "Source":
      return (
        <svg class="chip-glyph" aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
          <path d="M6 4.5 2.5 8 6 11.5M10 4.5 13.5 8 10 11.5" />
        </svg>
      );
  }
}

export function Chips() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const chips = runtime.scene.store.chips.value;
  const refs = useRef(new Map<number, HTMLDivElement>());
  const edgeRef = useRef<HTMLDivElement>(null);
  const glowRef = useRef<HTMLDivElement>(null);
  const write = useMemo(styleWriter, []);

  useFrame((at) => {
    const stage = runtime.stage;
    let glow = 0;
    for (const chip of chips) {
      const element = refs.current.get(chip.serial) ?? null;
      const originX = tokens.chipLeft;
      const originY = stage.height - tokens.chipBottom + chip.slot * tokens.chipGap;
      const targetX = stage.width - tokens.chipTargetRight + chip.slot * tokens.chipTargetStepX;
      const targetY = stage.height / 2 - tokens.chipTargetLift + chip.slot * tokens.chipTargetStepY;
      write(element, "left", `${originX}px`);
      write(element, "top", `${round1(originY)}px`);
      if (chip.kind === "Source") {
        const enter = easeOut(ramp(at, chip.at, chip.at + tokens.durationStandardMs));
        const leave = easeOut(ramp(at, chip.flyAt, chip.flyAt + tokens.durationStandardMs));
        write(element, "display", at >= chip.at && leave < 1 ? "flex" : "none");
        write(element, "opacity", String(round2(enter * (1 - leave))));
        write(element, "transform", `translateY(${round1(tokens.surfaceRise * (1 - enter))}px)`);
        write(element, "filter", `blur(${round1(8 * (1 - enter) + 14 * leave)}px)`);
        continue;
      }
      const flight = chipFlight(
        [originX, originY],
        [targetX, targetY],
        chip.at,
        chip.flyAt,
        at,
        tokens.durationStandardMs,
        tokens.durationChipFlyMs,
        tokens.surfaceRise,
      );
      write(element, "display", flight.visible ? "flex" : "none");
      write(element, "opacity", String(round2(flight.opacity)));
      write(element, "filter", `blur(${round1(flight.blur)}px)`);
      write(
        element,
        "transform",
        `translate(${round1(flight.x)}px, ${round1(flight.y)}px) scale(${round2(flight.scale)})`,
      );
      const landed = chip.flyAt + tokens.durationGlowDelayMs;
      glow = Math.max(glow, bump(at, landed - 150, landed + 120, landed + 500, landed + 1600));
    }
    write(edgeRef.current, "opacity", String(round2(0.45 + 0.55 * glow)));
    write(glowRef.current, "opacity", String(round2(glow)));
    write(glowRef.current, "transform", `scaleY(${round2(0.6 + 0.4 * glow)})`);
  });

  return (
    <>
      <div class="brain-edge" ref={edgeRef} aria-hidden="true" />
      <div class="brain-edge-glow" ref={glowRef} aria-hidden="true" />
      {chips.map((chip) => (
        <div
          key={chip.serial}
          class="chip"
          ref={(element) => {
            if (element === null) {
              refs.current.delete(chip.serial);
            } else {
              refs.current.set(chip.serial, element);
            }
          }}
        >
          <ChipGlyph kind={chip.kind} />
          <span class="chip-kind">{chip.kind}</span>
          <span class={chip.kind === "Source" ? "chip-text chip-source" : "chip-text"}>
            {chip.text}
          </span>
        </div>
      ))}
    </>
  );
}
