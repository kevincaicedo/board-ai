// The only control on screen (PRODUCT §8.7): a round mic button at bottom right. It rings with
// your voice while BoardAI listens and shows a slashed glyph when muted. Keyboard: hold Space to
// talk, M to mute, Esc to stop (PRODUCT §8.9; the mapping lives in keys.ts).

import { useEffect, useMemo, useRef } from "preact/hooks";
import { ramp, round1, round2 } from "../canvas/math.ts";
import { styleWriter, useFrame, useRuntime } from "./hooks.ts";
import { keyDownCommand } from "./keys.ts";
import { escapeAction } from "./replays.ts";

function editable(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || target.tagName === "INPUT" || target.tagName === "TEXTAREA")
  );
}

export function MicButton() {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const orb = runtime.scene.store.orb.value;
  const muted = orb.state === "Muted";
  const buttonRef = useRef<HTMLButtonElement>(null);
  const write = useMemo(styleWriter, []);

  useFrame((at) => {
    const listening = orb.state === "Listening";
    const live = listening
      ? ramp(at, orb.at, orb.at + tokens.durationMicOnMs)
      : orb.previous === "Listening"
        ? 1 - ramp(at, orb.at, orb.at + tokens.durationMicOffMs)
        : 0;
    const voice = listening ? runtime.voiceLevel : 0;
    const ring = live * (tokens.micRing + tokens.micRingVoice * voice);
    write(
      buttonRef.current,
      "box-shadow",
      ring < 0.05
        ? "var(--shadow-button)"
        : `var(--shadow-button), 0 0 0 ${round1(ring)}px rgba(var(--signal-rgb), ${round2(0.16 * live)})`,
    );
  });

  useEffect(() => {
    let talking = false;
    const down = (event: KeyboardEvent): void => {
      const closing = escapeAction(
        runtime.replays.local.value !== null,
        runtime.scene.views.open !== null,
      );
      const key = {
        code: event.code,
        repeat: event.repeat,
        modified: event.metaKey || event.ctrlKey || event.altKey,
        editable: editable(event.target),
      };
      const context = { muted, replay: runtime.replay !== null, escape: closing };
      const command = keyDownCommand(key, context);
      if (event.code === "Escape" && closing === "close-replay" && command === null) {
        if (!key.repeat && !key.modified && !key.editable) {
          event.preventDefault();
          runtime.replays.close();
        }
        return;
      }
      if (command === null) {
        return;
      }
      if (command.type === "Mic" && command.action === "TalkDown") {
        event.preventDefault();
        talking = true;
      }
      runtime.send(command);
    };
    const up = (event: KeyboardEvent): void => {
      if (event.code === "Space" && talking) {
        talking = false;
        runtime.send({ type: "Mic", action: "TalkUp" });
      }
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [runtime, muted]);

  return (
    <button
      type="button"
      class="mic"
      ref={buttonRef}
      aria-label={
        muted
          ? "Microphone muted"
          : orb.state === "Listening"
            ? "Microphone, hearing you"
            : "Microphone on"
      }
      aria-pressed={muted}
      onClick={() => runtime.send({ type: "Mic", action: muted ? "Unmute" : "Mute" })}
    >
      <svg aria-hidden="true" width="22" height="22" viewBox="0 0 24 24">
        <rect x="9" y="3" width="6" height="11" rx="3" />
        <path d="M5.5 11a6.5 6.5 0 0 0 13 0" />
        <path d="M12 17.5V21" />
        {muted ? <path d="M4 4l16 16" /> : null}
      </svg>
    </button>
  );
}
