// The small idle orb above the first-run title (artboard 11: 64 px, no dot field). The same
// renderer as the stage's orb, on its own frame loop, which rests while the window is hidden.

import { emptyParams, OrbDynamics } from "../orb/dynamics.ts";
import { OrbView } from "../orb/orb.ts";

export function mountSetupOrb(host: HTMLElement, size: number, still: boolean): () => void {
  const view = new OrbView(host, "BoardAI");
  const dynamics = new OrbDynamics();
  const params = emptyParams();
  const begin = performance.now();
  const frame = { x: size / 2, y: size / 2, size, opacity: 1, fieldScale: 0 };
  let request = 0;
  const draw = (now: number): void => {
    dynamics.sample(now - begin, null, null, params);
    view.render(frame, params, still);
    request = still || document.hidden ? 0 : requestAnimationFrame(draw);
  };
  const wake = (): void => {
    if (!document.hidden && request === 0 && !still) {
      request = requestAnimationFrame(draw);
    }
  };
  draw(begin);
  document.addEventListener("visibilitychange", wake);
  return () => {
    document.removeEventListener("visibilitychange", wake);
    cancelAnimationFrame(request);
    view.root.remove();
  };
}
