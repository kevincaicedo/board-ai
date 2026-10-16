import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { FakeTransport } from "../transport/fake.ts";
import { Runtime } from "./runtime.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

test("pointer targets go out once per change, and null when the pointer leaves", () => {
  const transport = new FakeTransport();
  const runtime = new Runtime(transport, null, tokens, false);
  runtime.pointer({ kind: "Element", layer: 0, id: "redis" });
  runtime.pointer({ kind: "Element", layer: 0, id: "redis" });
  runtime.pointer({ kind: "CodeLine", surface: 1, line: 43 });
  runtime.pointer(null);
  runtime.pointer(null);
  assert.deepEqual(transport.sent, [
    { type: "Pointer", target: { kind: "Element", layer: 0, id: "redis" } },
    { type: "Pointer", target: { kind: "CodeLine", surface: 1, line: 43 } },
    { type: "Pointer", target: null },
  ]);
});

test("a batch reaches the board, the orb and the surfaces; a reset clears them all", async () => {
  const transport = new FakeTransport();
  const runtime = new Runtime(transport, null, tokens, false);
  await transport.connect(runtime.scene);
  transport.push({
    at: 1000,
    events: [
      { type: "Orb", state: "Drawing", level_percent: 50 },
      { type: "Caption", who: "Assistant", text: "Sure", is_final: false },
      {
        type: "OpStart",
        layer: 0,
        budget_ms: 900,
        op: {
          kind: "Node",
          id: "lb",
          op: 1,
          shape: "Service",
          label: "LB",
          rect: { x: 0, y: 0, w: 120, h: 60 },
          stack: 1,
          fill: "Sky",
          link: null,
        },
      },
    ],
  });
  const scene = runtime.scene;
  assert.equal(scene.board.main.runs.length, 1);
  assert.equal(scene.presence.condenseAt, 1000);
  assert.equal(scene.store.captions.value.current?.text, "Sure");
  assert.equal(scene.orb.state, "Drawing");
  const generation = scene.generation;
  transport.reset();
  assert.equal(scene.board.main.runs.length, 0);
  assert.equal(scene.presence.condenseAt, null);
  assert.equal(scene.store.captions.value.current, null);
  assert.equal(scene.generation, generation + 1);
});
