import assert from "node:assert/strict";
import { test } from "node:test";
import { compareRows, compareTitle } from "./compare.ts";

const surface = {
  kind: "Compare" as const,
  id: 1,
  options: ["Raft", "Paxos", "ZAB"],
  criteria: ["Easy to follow", "Used by"],
  title: null,
};

test("criteria are rows and options are columns; a cell is keyed row:col", () => {
  const rows = compareRows(surface);
  assert.deepEqual(
    rows.map((row) => row.criterion),
    ["Easy to follow", "Used by"],
  );
  // The core's Cell { row: 1, col: 2 } is "Used by" for ZAB.
  const cell = rows[1]?.cells[2];
  assert.deepEqual(cell, { option: "ZAB", col: 2, key: "1:2" });
});

test("the title heads the card, or the options joined by vs", () => {
  assert.equal(compareTitle(surface), "Raft vs Paxos vs ZAB");
  assert.equal(compareTitle({ ...surface, title: "Consensus" }), "Consensus");
});
