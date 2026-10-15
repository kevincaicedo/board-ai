// Which transport the UI starts with. Inside Tauri it is the live session; `?replay=<url>` plays a
// file; outside Tauri with no file it plays the design's demo, so `npm run dev` shows the demo.

import { REPLAY_SPEEDS, type ReplaySpeed } from "./replay.ts";

export const DEMO_REPLAY_URL = "/fixtures/demo.events.jsonl";

export type TransportChoice =
  | { readonly kind: "tauri" }
  | {
      readonly kind: "replay";
      readonly url: string;
      /** `&t=<ms>`: open paused at this session time (used for screenshots). */
      readonly seekMs: number | null;
      readonly speed: ReplaySpeed;
    };

export function chooseTransport(search: string, insideTauri: boolean): TransportChoice {
  const params = new URLSearchParams(search);
  const url = params.get("replay");
  if (url === null && insideTauri) {
    return { kind: "tauri" };
  }
  const rawSeek = params.get("t");
  const seek = rawSeek === null || rawSeek.trim() === "" ? Number.NaN : Number(rawSeek);
  const rawSpeed = Number(params.get("speed") ?? "1");
  const speed = REPLAY_SPEEDS.find((candidate) => candidate === rawSpeed) ?? 1;
  return {
    kind: "replay",
    url: url === null || url.trim() === "" ? DEMO_REPLAY_URL : url,
    seekMs: Number.isFinite(seek) && seek >= 0 ? Math.floor(seek) : null,
    speed,
  };
}
