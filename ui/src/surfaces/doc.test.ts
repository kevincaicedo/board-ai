import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { surfaceHeight } from "../app/layout.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import {
  type DocSurface,
  docCardHeight,
  docSentences,
  FIGURE_UNREAD,
  figureLines,
  sentenceMarks,
} from "./doc.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

/** Artboard 14b's card: page 3's figure, read by the talker model. */
const FIGURE: DocSurface = {
  kind: "Doc",
  id: 0,
  title: "FoundationDB · SIGMOD ’21",
  chip: "p. 3 · Figure 1",
  figure: {
    label: "Figure 1",
    caption: "The architecture and the transaction processing of FoundationDB.",
    read: true,
  },
  sentences: [
    "FoundationDB unbundles its transaction system from its storage system.",
    "A client first asks a proxy for a read version.",
    "Same sentence.",
    "Same sentence.",
  ],
  highlight: 0,
  note: "Figure read by Gemini 3.8 Flash. Redrawn on the board from the digest.",
};

const PASSAGE: DocSurface = { ...FIGURE, chip: "pp. 3–4", figure: null };

test("the figure box names a figure, and says when it needs a model that reads files", () => {
  assert.deepEqual(figureLines(FIGURE), {
    label: "Figure 1",
    caption: "The architecture and the transaction processing of FoundationDB.",
    status: null,
  });
  const unread: DocSurface = {
    ...FIGURE,
    figure: { label: "Figure 4", caption: "Recovery.", read: false },
  };
  assert.equal(figureLines(unread)?.status, FIGURE_UNREAD);
  assert.equal(figureLines(PASSAGE), null);
});

test("sentences keep their place: one repeated word for word gets its own key", () => {
  const sentences = docSentences(FIGURE);
  assert.deepEqual(
    sentences.map((sentence) => [sentence.index, sentence.key]),
    [
      [0, "sentence-0"],
      [1, "sentence-1"],
      [2, "sentence-2"],
      [3, "sentence-3"],
    ],
  );
});

test("the highlight eases from the previous sentence to the new one", () => {
  const moves = [
    { index: 0, at: 1000 },
    { index: 1, at: 2000 },
  ];
  const duration = 420;
  assert.deepEqual(sentenceMarks([], 4, 5000, duration), []);
  assert.deepEqual(sentenceMarks(moves, 4, 500, duration), []);
  // The first move fades in on its own.
  const first = sentenceMarks(moves, 4, 1000 + duration / 2, duration);
  assert.equal(first.length, 1);
  assert.equal(first[0]?.index, 0);
  assert.ok((first[0]?.strength ?? 0) > 0 && (first[0]?.strength ?? 1) < 1);
  // Midway through the second, the first fades out as the second fades in.
  const mid = sentenceMarks(moves, 4, 2000 + duration / 2, duration);
  assert.deepEqual(
    mid.map((mark) => mark.index),
    [0, 1],
  );
  const [out, into] = mid;
  assert.ok(out !== undefined && into !== undefined);
  assert.ok(Math.abs(out.strength + into.strength - 1) < 1e-9);
  // Settled, only the new sentence is marked, fully.
  assert.deepEqual(sentenceMarks(moves, 4, 2000 + duration, duration), [{ index: 1, strength: 1 }]);
  // A seek back lands on the moves made by then.
  assert.deepEqual(sentenceMarks(moves, 4, 1999, duration), [{ index: 0, strength: 1 }]);
});

test("a repeated move holds its sentence, and an index past the passage marks nothing", () => {
  const same = [
    { index: 2, at: 1000 },
    { index: 2, at: 1500 },
  ];
  assert.deepEqual(sentenceMarks(same, 4, 1600, 420), [{ index: 2, strength: 1 }]);
  assert.deepEqual(sentenceMarks([{ index: 9, at: 0 }], 4, 1000, 420), []);
  assert.deepEqual(sentenceMarks([{ index: 1, at: 0 }], 4, 1000, 0), [{ index: 1, strength: 1 }]);
});

test("the card's height counts the figure box, the passage's lines and the note", () => {
  const withFigure = docCardHeight(FIGURE, tokens);
  const passage = docCardHeight(PASSAGE, tokens);
  assert.ok(withFigure > passage, "the figure box takes room");
  const longer = docCardHeight(
    { ...PASSAGE, sentences: [...PASSAGE.sentences, "x".repeat(tokens.docCharsPerLine * 3)] },
    tokens,
  );
  assert.ok(longer >= passage + 3 * tokens.docLine, "each wrapped line takes a line's height");
  const bare: DocSurface = { ...PASSAGE, sentences: [], note: "" };
  assert.equal(docCardHeight(bare, tokens), 2 * tokens.docPadding + tokens.docHead);
  // The column sizes a doc card the same way.
  assert.equal(surfaceHeight(FIGURE, tokens), withFigure);
});
