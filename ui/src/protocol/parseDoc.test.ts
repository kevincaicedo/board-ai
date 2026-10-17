// The Doc card at the boundary (D-257): a well-formed card, one with no figure and nothing yet to
// say, and the malformed ones a replay of another build might hold, each rejected, never thrown.

import assert from "node:assert/strict";
import { test } from "node:test";
import { parseUiEvent } from "./parse.ts";

test("a Doc card with no figure and no passage yet parses", () => {
  const card = {
    type: "SurfaceShow",
    surface: {
      kind: "Doc",
      id: 6,
      title: "hekaton-sigmod13.pdf",
      chip: "pp. 3–4",
      figure: null,
      sentences: [],
      highlight: null,
      note: "",
    },
  };
  assert.deepEqual(parseUiEvent(card), card);
});

test("a malformed Doc card or Sentence update is rejected", () => {
  const bad: unknown[] = [
    // A Doc card missing its note, with a figure that isn't one, a highlight that isn't a
    // sentence index, or a sentence that isn't text; a Sentence update with no index.
    {
      type: "SurfaceShow",
      surface: { kind: "Doc", id: 5, title: "t", chip: "p. 3", figure: null, sentences: [] },
    },
    {
      type: "SurfaceShow",
      surface: {
        kind: "Doc",
        id: 5,
        title: "t",
        chip: "p. 3",
        figure: { label: "Figure 1", caption: "c" },
        sentences: [],
        highlight: null,
        note: "",
      },
    },
    {
      type: "SurfaceShow",
      surface: {
        kind: "Doc",
        id: 5,
        title: "t",
        chip: "p. 3",
        figure: null,
        sentences: ["a"],
        highlight: -1,
        note: "",
      },
    },
    {
      type: "SurfaceShow",
      surface: {
        kind: "Doc",
        id: 5,
        title: "t",
        chip: "p. 3",
        figure: null,
        sentences: [3],
        highlight: null,
        note: "",
      },
    },
    { type: "SurfaceUpdate", id: 5, update: { kind: "Sentence" } },
    { type: "SurfaceUpdate", id: 5, update: { kind: "Sentence", index: 1.5 } },
  ];
  for (const value of bad) {
    assert.equal(parseUiEvent(value), null, JSON.stringify(value));
  }
});
