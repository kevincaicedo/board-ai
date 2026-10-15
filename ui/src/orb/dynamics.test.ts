import assert from "node:assert/strict";
import { test } from "node:test";
import { emptyParams, OrbDynamics, voiceTexture } from "./dynamics.ts";

const at = (
  dynamics: OrbDynamics,
  time: number,
  freezeAt: number | null = null,
  ackAt: number | null = null,
) => dynamics.sample(time, freezeAt, ackAt, emptyParams());

test("state targets are approached exponentially, as the design eased them per frame", () => {
  const dynamics = new OrbDynamics();
  dynamics.set("Listening", 0, 1000);
  assert.equal(at(dynamics, 1000).swell, 1);
  // The design's per-frame k = 1 - e^(-9·dt): after 1/9 s, 1 - 1/e of the way.
  const third = at(dynamics, 1000 + 1000 / 9).swell;
  assert.ok(Math.abs(third - (1 + 0.08 * (1 - Math.exp(-1)))) < 1e-9);
  assert.ok(Math.abs(at(dynamics, 4000).swell - 1.08) < 1e-6);
  assert.ok(at(dynamics, 4000).field > 0.999);
});

test("sampling is a pure function of time, so a seek lands on the same frame", () => {
  const live = new OrbDynamics();
  live.set("Thinking", 0, 500);
  live.set("Speaking", 70, 2000);
  const frames = [600, 1200, 2100, 5000].map((time) => at(live, time));
  const replayed = new OrbDynamics();
  replayed.set("Thinking", 0, 500);
  replayed.set("Speaking", 70, 2000);
  assert.deepEqual([5000, 2100, 1200, 600].map((time) => at(replayed, time)).reverse(), frames);
});

test("the swirl phase keeps turning and turns faster while thinking", () => {
  const idle = new OrbDynamics();
  const thinking = new OrbDynamics();
  thinking.set("Thinking", 0, 0);
  const idlePhase = at(idle, 2000).phase;
  const thinkingPhase = at(thinking, 2000).phase;
  assert.ok(idlePhase > 0.69 && idlePhase < 0.71, `idle turns at 0.35/s: ${idlePhase}`);
  assert.ok(thinkingPhase > 4 * idlePhase);
  // Continuity across a state change: no jump in phase.
  thinking.set("Idle", 0, 2000);
  assert.ok(Math.abs(at(thinking, 2000).phase - thinkingPhase) < 1e-9);
});

test("muted slows the orb's clock to a crawl and desaturates", () => {
  const dynamics = new OrbDynamics();
  dynamics.set("Muted", 0, 0);
  const late = at(dynamics, 20_000);
  assert.ok(late.mute > 0.999);
  assert.ok(late.clock < 0.2 * 20 + 0.2);
});

test("the voice level drives amplitude with the voice's texture", () => {
  const dynamics = new OrbDynamics();
  dynamics.set("Speaking", 100, 0);
  const loud = at(dynamics, 2000).amp;
  assert.ok(Math.abs(loud - Math.min(1, voiceTexture(2))) < 1e-6);
  dynamics.set("Speaking", 0, 2000);
  assert.ok(at(dynamics, 2500).amp < 0.01, "the release takes well under half a second");
});

test("an interruption contracts and cools the orb, and an acknowledgement pulses it", () => {
  const dynamics = new OrbDynamics();
  const calm = at(dynamics, 1000);
  const contracted = at(dynamics, 1090, 1000);
  assert.ok(contracted.swell < calm.swell * 0.75);
  assert.ok(contracted.cool > 0.7 && contracted.lean > 2);
  const recovered = at(dynamics, 4000, 1000);
  assert.equal(recovered.cool, 0);
  assert.ok(at(dynamics, 1150, null, 1000).swell > calm.swell);
});
