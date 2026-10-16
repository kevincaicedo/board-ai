import assert from "node:assert/strict";
import { test } from "node:test";
import { lineRange, tokenizeLine } from "./code.ts";
import { markStrokes } from "./marks.ts";

test("keywords, comments and numbers are marked; everything else stays plain text", () => {
  const tokens = tokenizeLine("        return u, nil // cache hit");
  assert.deepEqual(tokens, [
    { kind: "plain", text: "        " },
    { kind: "keyword", text: "return" },
    { kind: "plain", text: " u, nil " },
    { kind: "comment", text: "// cache hit" },
  ]);
  const numbers = tokenizeLine("s.cache.Set(ctx, userKey(id), u, 10*time.Minute)");
  assert.deepEqual(
    numbers.filter((token) => token.kind !== "plain"),
    [{ kind: "number", text: "10" }],
  );
  assert.equal(tokenizeLine("f((// x").at(-1)?.kind, "comment");
  assert.equal(
    tokenizeLine("#[derive(Debug)]").every((token) => token.kind === "plain"),
    true,
  );
});

test("tokens always reassemble into the original line", () => {
  for (const line of [
    "",
    "  x := a[1:2] # comment",
    "fn main() { let x = 3.14; }",
    "∑ unicode → ok",
  ]) {
    assert.equal(
      tokenizeLine(line)
        .map((token) => token.text)
        .join(""),
      line,
    );
  }
});

// Review finding 8: `\b\d+\b` failed before a letter, and no other branch matched a digit, so
// the card silently dropped digits that touch letters (0xFF read as xFF, 100ms as ms).
test("the code card keeps every character of a line, digits touching letters included", () => {
  for (const line of [
    "const mask = 0xFF;",
    "let timeout = 100ms;",
    "x := 3rd + 1e9",
    "width: 2px;",
    "id := uuid4()",
    "s.cache.Get(ctx, key)",
  ]) {
    assert.equal(
      tokenizeLine(line)
        .map((token) => token.text)
        .join(""),
      line,
    );
  }
  assert.deepEqual(
    tokenizeLine("x = 0xFF + 2px").filter((token) => token.kind === "number"),
    [
      { kind: "number", text: "0xFF" },
      { kind: "number", text: "2px" },
    ],
  );
});

test("tokens reassemble any line, whatever characters it holds", () => {
  // A seeded generator over the characters code lines mix: letters, digits, punctuation,
  // comment starts, whitespace and non-ASCII.
  const alphabet = "aZ_09x.e#/ \t-+()[]{}:;'\"`é→∑😀\\";
  const letters = Array.from(alphabet);
  let seed = 7;
  const next = (): number => {
    seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
    return seed;
  };
  for (let round = 0; round < 2000; round += 1) {
    const length = next() % 24;
    let line = "";
    for (let index = 0; index < length; index += 1) {
      line += letters[next() % letters.length] ?? "";
    }
    assert.equal(
      tokenizeLine(line)
        .map((token) => token.text)
        .join(""),
      line,
      JSON.stringify(line),
    );
  }
});

test("the path chip names the line range like the design", () => {
  assert.equal(lineRange(42, 11), "L42–52");
  assert.equal(lineRange(7, 1), "L7");
});

test("compare marks are hand-jittered but stable per cell", () => {
  const first = markStrokes("Yes", "2:0:1").map((stroke) => stroke.path);
  assert.deepEqual(
    markStrokes("Yes", "2:0:1").map((stroke) => stroke.path),
    first,
  );
  assert.notDeepEqual(
    markStrokes("Yes", "2:1:1").map((stroke) => stroke.path),
    first,
  );
  assert.equal(markStrokes("No", "x").length, 2);
  assert.equal(markStrokes("No", "x")[0]?.tone, "ink3");
});
