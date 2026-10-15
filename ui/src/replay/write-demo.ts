// Writes the replay fixtures: `node src/replay/write-demo.ts` from ui/. The tests in
// demo.test.ts fail while a committed fixture differs from what this writes.

import { writeFileSync } from "node:fs";
import { demoJsonl } from "./demo.ts";
import { primitivesJsonl } from "./primitives.ts";

for (const [name, text] of [
  ["demo", demoJsonl()],
  ["primitives", primitivesJsonl()],
] as const) {
  const target = new URL(`../../public/fixtures/${name}.events.jsonl`, import.meta.url);
  writeFileSync(target, text);
  console.info(`wrote ${target.pathname}`);
}
