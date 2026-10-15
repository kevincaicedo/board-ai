import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { MissingTokensError, readTokens, tokenSourceFromCss } from "./tokens.ts";

const css = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");

test("every numeric token the renderers need is declared in tokens.css", () => {
  const tokens = readTokens(tokenSourceFromCss(css));
  assert.equal(tokens.orbSize, 236);
  assert.equal(tokens.durationStandardMs, 420);
  assert.equal(tokens.detourPushShrink, 0.035);
});

test("tokens read the same after the production build minifies them", () => {
  // Vite's CSS minifier writes 900ms as .9s and 0.035 as .035; a strict reader blanked the app.
  const minified = css
    .replace("--duration-scene: 900ms;", "--duration-scene: .9s;")
    .replace("--duration-micro: 160ms;", "--duration-micro: .16s;")
    .replace("--duration-standard: 420ms;", "--duration-standard: 0s;")
    .replace("--detour-push-shrink: 0.035;", "--detour-push-shrink: .035;");
  const tokens = readTokens(tokenSourceFromCss(minified));
  assert.equal(tokens.durationSceneMs, 900);
  assert.equal(tokens.durationMicroMs, 160);
  assert.equal(tokens.durationStandardMs, 0);
  assert.equal(tokens.detourPushShrink, 0.035);
});

test("a missing or malformed token fails loudly with its name", () => {
  const broken = css
    .replace("--orb-size: 236px;", "")
    .replace("--duration-scene: 900ms;", "--duration-scene: fast;");
  assert.throws(
    () => readTokens(tokenSourceFromCss(broken)),
    (error: unknown) =>
      error instanceof MissingTokensError &&
      error.names.includes("--orb-size") &&
      error.names.includes("--duration-scene"),
  );
});

test("no component or renderer source carries a raw hex color", async () => {
  const { readdirSync, statSync } = await import("node:fs");
  const { join } = await import("node:path");
  const root = new URL("..", import.meta.url).pathname;
  const offenders: string[] = [];
  const walk = (directory: string): void => {
    for (const name of readdirSync(directory)) {
      const path = join(directory, name);
      if (statSync(path).isDirectory()) {
        if (name !== "generated") {
          walk(path);
        }
      } else if (/\.(ts|tsx)$/.test(name) && !name.endsWith(".test.ts")) {
        const text = readFileSync(path, "utf8");
        if (/#[0-9a-fA-F]{3,8}\b/.test(text.replace(/\/\/.*$/gm, ""))) {
          offenders.push(path.slice(root.length));
        }
      }
    }
  };
  walk(root);
  assert.deepEqual(offenders, []);
});
