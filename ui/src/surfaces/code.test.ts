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
