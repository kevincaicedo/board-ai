// Preact keys on the cards' lists (review, low): keys made from what a list says repeat when
// the talker repeats itself (two options named "Redis", the same bullet twice, a second pen mark
// on the same characters), and Preact then reuses or drops the wrong element. A card's lists
// never reorder once shown, so position (or arrival, for pen marks) is the identity.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { SessionStore } from "../app/session.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import { codeMarkKey } from "./codeMark.ts";
import { compareOptions, compareRows } from "./compare.ts";
import { recapRows } from "./recap.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

const unique = (keys: readonly string[]): boolean => new Set(keys).size === keys.length;

test("compare options, criteria and cells keep their own keys when names repeat", () => {
  const surface = {
    kind: "Compare" as const,
    id: 3,
    options: ["Redis", "Redis", "Postgres"],
    criteria: ["Fast", "Fast"],
    title: null,
  };
  const rows = compareRows(surface);
  const keys = [
    ...compareOptions(surface).map((option) => option.key),
    ...rows.map((row) => row.key),
    ...rows.flatMap((row) => row.cells.map((cell) => cell.key)),
  ];
  assert.equal(keys.length, 3 + 2 + 6);
  assert.ok(unique(keys), keys.join(" "));
});

test("recap bullets keep their own keys when a bullet repeats", () => {
  const rows = recapRows({ kind: "Recap", id: 4, bullets: ["TTLs", "TTLs", "Reads hit Redis"] });
  assert.deepEqual(
    rows.map((row) => row.text),
    ["TTLs", "TTLs", "Reads hit Redis"],
  );
  assert.ok(unique(rows.map((row) => row.key)));
});

test("every pen mark on a code card has its own key, even on the same characters", () => {
  const store = new SessionStore(tokens);
  store.apply(
    {
      type: "SurfaceShow",
      surface: { kind: "Code", id: 1, path: "a.go", first_line: 1, lines: ["x"], highlight: null },
    },
    0,
  );
  const mark = (at: number, kind: "Underline" | "Circle"): void =>
    store.apply(
      {
        type: "SurfaceUpdate",
        id: 1,
        update: { kind: "Mark", line: 1, columns: { begin: 0, end: 1 }, mark: kind },
      },
      at,
    );
  // More than the card keeps, so the oldest are dropped while new ones arrive.
  for (let index = 0; index < 40; index += 1) {
    mark(100 + index, index % 2 === 0 ? "Underline" : "Circle");
  }
  const card = store.column.value[0];
  const keys = card?.kind === "surface" ? card.marks.map(codeMarkKey) : [];
  assert.ok(keys.length > 1);
  assert.ok(unique(keys), keys.join(" "));
});
