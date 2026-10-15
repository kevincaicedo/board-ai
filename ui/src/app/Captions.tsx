// Live captions under the orb (PRODUCT §8.3): the current line in ink-2 (yours in ink-3, quoted),
// the one before it smaller and fading. The newest word fades in as it is spoken.

import { useMemo, useRef } from "preact/hooks";
import { clamp, easeOut, lerp, ramp } from "../canvas/math.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";
import { captionText } from "./layout.ts";
import type { CaptionLine } from "./session.ts";

function quoted(line: CaptionLine, text: string, isLast: boolean): string {
  if (line.who !== "User") {
    return text;
  }
  return isLast ? `${text}${line.finalAt !== null ? "”" : ""}` : `“${text}`;
}

export function Captions() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const { current, previous } = runtime.scene.store.captions.value;
  const currentRef = useRef<HTMLDivElement>(null);
  const lastWordRef = useRef<HTMLSpanElement>(null);
  const previousRef = useRef<HTMLDivElement>(null);
  const write = useMemo(styleWriter, []);

  useFrame((at) => {
    if (current !== null) {
      const enter = easeOut(ramp(at, current.startAt, current.startAt + tokens.durationStandardMs));
      const lingerEnd =
        current.finalAt === null
          ? Number.POSITIVE_INFINITY
          : current.finalAt + tokens.durationCaptionLingerMs;
      const fade = 1 - ramp(at, lingerEnd, lingerEnd + tokens.durationCaptionFadeMs);
      write(currentRef.current, "opacity", String(Math.round(fade * enter * 100) / 100));
      write(
        currentRef.current,
        "transform",
        `translateY(${Math.round(tokens.captionRise * (1 - enter) * 10) / 10}px)`,
      );
      const word = clamp((at - current.wordAt) / tokens.durationCaptionWordMs);
      write(lastWordRef.current, "opacity", String(Math.round((0.25 + 0.75 * word) * 100) / 100));
      if (previous !== null) {
        const begin = current.startAt + tokens.durationCaptionPreviousLingerMs;
        const opacity =
          (1 - ramp(at, begin, begin + tokens.durationCaptionPreviousFadeMs)) *
          lerp(1, 0.85, enter);
        write(previousRef.current, "opacity", String(Math.round(opacity * 100) / 100));
        write(
          previousRef.current,
          "transform",
          `translateY(${Math.round(-tokens.captionPreviousRise * enter * 10) / 10}px)`,
        );
        write(
          previousRef.current,
          "font-size",
          `${Math.round(lerp(tokens.typeCaption, tokens.typeCaptionPrevious, enter) * 10) / 10}px`,
        );
      }
    }
  });

  if (current === null) {
    return null;
  }
  const max = tokens.captionMaxLetters;
  const now = captionText(current.text, max);
  const before =
    previous === null
      ? null
      : current.detail !== null
        ? { head: "", last: current.detail }
        : captionText(previous.text, max);
  return (
    <div class="captions">
      {before !== null && previous !== null ? (
        <div class="caption caption-previous" ref={previousRef}>
          <span>
            {current.detail !== null ? before.last : quoted(previous, before.head, false)}
          </span>
          {current.detail !== null ? null : <span>{quoted(previous, before.last, true)}</span>}
        </div>
      ) : null}
      <div
        class={`caption caption-current ${current.who === "User" ? "caption-user" : ""}`}
        ref={currentRef}
        aria-live="polite"
      >
        <span>{quoted(current, now.head, false)}</span>
        <span ref={lastWordRef}>{quoted(current, now.last, true)}</span>
      </div>
    </div>
  );
}
