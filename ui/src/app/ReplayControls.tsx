// Replay controls at bottom left (the live demo's): play, pause or replay, a scrubber with a dot
// per chapter, the time and the current chapter. Click or drag the track to seek.

import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { round1 } from "../canvas/math.ts";
import type { ChapterKind } from "../transport/log.ts";
import type { ReplayTransport } from "../transport/replay.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";

function clock(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

const CHAPTER_NAMES: { readonly [K in ChapterKind]: string } = {
  Idle: "Idle",
  Listening: "Listening",
  Explaining: "Explaining",
  Detour: "Detour",
  Code: "Code trace",
  Compare: "Compare",
  Recap: "Recap",
  Capture: "Capture",
  Resume: "Resume",
};

export function ReplayControls({
  replay,
  keyboard = true,
  trailing = null,
}: {
  replay: ReplayTransport;
  /** A replay inside the live app leaves Space to push-to-talk. */
  keyboard?: boolean;
  /** Ends the row: a session's replay closes from here (7b). */
  trailing?: ComponentChildren;
}) {
  const runtime = useRuntime();
  const [state, setState] = useState(replay.state);
  const progressRef = useRef<HTMLDivElement>(null);
  const timeRef = useRef<HTMLSpanElement>(null);
  const chapterRef = useRef<HTMLSpanElement>(null);
  const dotRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const trackRef = useRef<HTMLDivElement>(null);
  const write = useMemo(styleWriter, []);
  const { durationMs, chapters } = state;

  useEffect(() => {
    // The replay may have started between render and here (a replay opened from a card).
    setState(replay.state);
    return replay.subscribe(() => setState(replay.state));
  }, [replay]);

  // In a replay there is no microphone, so Space plays and pauses, like a video.
  useEffect(() => {
    if (!keyboard) {
      return;
    }
    const key = (event: KeyboardEvent): void => {
      if (event.code === "Space" && !event.repeat && event.target === document.body) {
        event.preventDefault();
        replay.toggle(performance.now());
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [replay, keyboard]);

  useFrame((at) => {
    write(progressRef.current, "width", `${round1((100 * at) / durationMs)}%`);
    let index = 0;
    chapters.forEach((chapter, chapterIndex) => {
      if (at >= chapter.at) {
        index = chapterIndex;
      }
    });
    const seconds = String(Math.round(at / 1000));
    if (trackRef.current !== null && trackRef.current.getAttribute("aria-valuenow") !== seconds) {
      trackRef.current.setAttribute("aria-valuenow", seconds);
    }
    const time = `${clock(at)} / ${clock(durationMs)}`;
    if (timeRef.current !== null && timeRef.current.textContent !== time) {
      timeRef.current.textContent = time;
    }
    const chapter = chapters[index];
    const name = chapter === undefined ? "" : `${index + 1} · ${CHAPTER_NAMES[chapter.kind]}`;
    if (chapterRef.current !== null && chapterRef.current.textContent !== name) {
      chapterRef.current.textContent = name;
    }
    dotRefs.current.forEach((dot, dotIndex) => {
      dot?.classList.toggle("current", dotIndex === index);
    });
  });

  const atEnd = !state.playing && runtime.at >= durationMs - 10;
  const label = state.playing ? "Pause replay" : atEnd ? "Replay from the start" : "Play replay";
  const seekFrom = (event: PointerEvent): void => {
    const track = trackRef.current;
    if (track === null) {
      return;
    }
    const box = track.getBoundingClientRect();
    const fraction = Math.min(1, Math.max(0, (event.clientX - box.left) / box.width));
    replay.seek(fraction * durationMs, performance.now());
  };

  return (
    <div class="replay">
      <button
        type="button"
        class="replay-button"
        aria-label={label}
        onClick={() => replay.toggle(performance.now())}
      >
        {state.playing ? (
          <svg aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
            <rect x="3.5" y="2.5" width="3" height="11" rx="1.2" />
            <rect x="9.5" y="2.5" width="3" height="11" rx="1.2" />
          </svg>
        ) : atEnd ? (
          <svg class="replay-again" aria-hidden="true" width="18" height="18" viewBox="0 0 18 18">
            <path d="M3.5 9a5.5 5.5 0 1 0 1.8-4.1" />
            <path d="M3.2 2.6v3h3" />
          </svg>
        ) : (
          <svg aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
            <path d="M4.5 2.8v10.4c0 .6.6.9 1.1.6l8-5.2c.5-.3.5-.9 0-1.2l-8-5.2c-.5-.3-1.1 0-1.1.6z" />
          </svg>
        )}
      </button>
      <div
        class="replay-track"
        ref={trackRef}
        role="slider"
        tabIndex={0}
        aria-label="Replay position"
        aria-valuenow={Math.round(runtime.at / 1000)}
        aria-valuemin={0}
        aria-valuemax={Math.round(durationMs / 1000)}
        onPointerDown={(event) => {
          (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
          seekFrom(event);
        }}
        onPointerMove={(event) => {
          if ((event.currentTarget as HTMLElement).hasPointerCapture(event.pointerId)) {
            seekFrom(event);
          }
        }}
        onKeyDown={(event) => {
          const step = event.key === "ArrowRight" ? 5000 : event.key === "ArrowLeft" ? -5000 : 0;
          if (step !== 0) {
            replay.seek(runtime.at + step, performance.now());
          }
        }}
      >
        <div class="replay-progress" ref={progressRef} />
        {chapters.map((chapter, index) => (
          <button
            // Chapters never reorder, and two can share a time and kind (a detour opened,
            // closed and opened again in one batch): the position is the key.
            key={`chapter-${index}`}
            type="button"
            class="replay-chapter"
            ref={(element) => {
              dotRefs.current[index] = element;
            }}
            style={{ left: `${(100 * chapter.at) / durationMs}%` }}
            aria-label={`Jump to ${index + 1}, ${CHAPTER_NAMES[chapter.kind]}`}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => replay.seek(chapter.at, performance.now())}
          >
            <span />
          </button>
        ))}
      </div>
      <span class="replay-time" ref={timeRef} />
      <span class="replay-chapter-name" ref={chapterRef} />
      {trailing}
    </div>
  );
}
