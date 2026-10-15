import assert from "node:assert/strict";
import { test } from "node:test";
import {
  endTime,
  freezeClock,
  isFrozen,
  naturalTime,
  OP_FREEZES_MAX,
  resumeClock,
  startClock,
} from "./timeline.ts";

test("an op that fits its budget runs at natural pace", () => {
  const clock = startClock(1000, 800, 2000);
  assert.equal(naturalTime(clock, 999), -1);
  assert.equal(naturalTime(clock, 1000), 0);
  assert.equal(naturalTime(clock, 1400), 400);
  assert.equal(naturalTime(clock, 5000), 800);
  assert.equal(endTime(clock), 1800);
});

test("an op longer than its budget is compressed to finish within it", () => {
  const clock = startClock(0, 1500, 1000);
  assert.equal(naturalTime(clock, 500), 750);
  assert.equal(endTime(clock), 1000);
  assert.equal(naturalTime(clock, 1000), 1500);
});

test("a zero budget draws the op at once", () => {
  const clock = startClock(0, 500, 0);
  assert.equal(naturalTime(clock, 0), 500);
});

test("freeze holds the stroke in place and resume continues from exactly there", () => {
  const clock = startClock(0, 1000, 2500);
  assert.equal(freezeClock(clock, 400), true);
  assert.equal(isFrozen(clock, 401), true);
  assert.equal(naturalTime(clock, 5000), 400);
  assert.equal(endTime(clock), null);
  assert.equal(resumeClock(clock, 10_000, 2500), true);
  assert.equal(naturalTime(clock, 10_000), 400);
  assert.equal(naturalTime(clock, 10_300), 700);
  assert.equal(endTime(clock), 10_600);
  // History before the freeze is unchanged, so a replay seek lands on the same frame.
  assert.equal(naturalTime(clock, 200), 200);
});

test("resume fits what is left into the new budget", () => {
  const clock = startClock(0, 1000, 2500);
  freezeClock(clock, 200);
  resumeClock(clock, 1000, 400);
  assert.equal(naturalTime(clock, 1200), 600);
  assert.equal(endTime(clock), 1400);
});

test("freeze and resume are ignored when they don't apply", () => {
  const clock = startClock(100, 500, 1000);
  assert.equal(freezeClock(clock, 50), false, "not started yet");
  assert.equal(resumeClock(clock, 200, 1000), false, "not frozen");
  assert.equal(freezeClock(clock, 700), false, "already done");
  assert.equal(naturalTime(clock, 700), 500);
});

test("freezes per op are bounded", () => {
  const clock = startClock(0, 1_000_000, 10_000_000);
  let accepted = 0;
  for (let index = 0; index < OP_FREEZES_MAX * 3; index += 1) {
    if (freezeClock(clock, index * 10 + 1)) {
      accepted += 1;
    }
    resumeClock(clock, index * 10 + 5, 10_000_000);
  }
  assert.ok(accepted < OP_FREEZES_MAX * 3);
  assert.ok(clock.segments.length <= OP_FREEZES_MAX * 2 + 1);
});
