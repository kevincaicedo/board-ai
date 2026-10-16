import assert from "node:assert/strict";
import { test } from "node:test";
import { codeMarkSeed, codeMarkStroke, utf16Offset } from "./codeMark.ts";

const box = { x: 202, y: 85, w: 77, h: 15 };
const mark = {
  line: 43,
  columns: { begin: 17, end: 29 },
  mark: "Underline" as const,
  at: 0,
  serial: 0,
};

test("an underline runs under the marked text in the pen's tone, the same on every replay", () => {
  const seed = codeMarkSeed(1, mark);
  const stroke = codeMarkStroke("Underline", box, seed);
  assert.equal(stroke.tone, "annotate");
  assert.equal(stroke.width, 2.2);
  assert.equal(stroke.path, codeMarkStroke("Underline", box, seed).path);
  const xs = Array.from(
    { length: stroke.points.length / 2 },
    (_, index) => stroke.points[index * 2] ?? 0,
  );
  const ys = Array.from(
    { length: stroke.points.length / 2 },
    (_, index) => stroke.points[index * 2 + 1] ?? 0,
  );
  assert.ok(Math.min(...xs) < box.x + 4 && Math.max(...xs) > box.x + box.w - 4, "spans the text");
  assert.ok(
    Math.min(...ys) > box.y + box.h - 5 && Math.max(...ys) < box.y + box.h + 3,
    "sits under it",
  );
  for (const other of ["Arrow", "Bracket"] as const) {
    assert.equal(codeMarkStroke(other, box, seed).kind, "line", `${other} underlines`);
  }
});

test("a circle goes around the marked text", () => {
  const stroke = codeMarkStroke("Circle", box, "c");
  assert.equal(stroke.kind, "ellipse");
  const xs = Array.from(
    { length: stroke.points.length / 2 },
    (_, index) => stroke.points[index * 2] ?? 0,
  );
  assert.ok(Math.min(...xs) < box.x && Math.max(...xs) > box.x + box.w);
});

test("columns count characters, not UTF-16 units", () => {
  assert.equal(utf16Offset("    if u", 4), 4);
  assert.equal(utf16Offset("😀 s.cache", 2), 3);
  assert.equal(utf16Offset("ab", 9), 2, "past the end clamps");
});
