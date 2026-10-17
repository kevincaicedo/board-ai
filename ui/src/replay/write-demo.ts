// Writes the replay fixtures: `node src/replay/write-demo.ts` from ui/. The tests in
// demo.test.ts fail while a committed fixture differs from what this writes.

import { mkdirSync, writeFileSync } from "node:fs";
import { demoJsonl } from "./demo.ts";
import { diagramsJsonl } from "./diagrams.ts";
import { drawingJsonl } from "./drawing.ts";
import { primitivesJsonl } from "./primitives.ts";
import { sourcesJsonl } from "./sources.ts";
import { sessionFixtures, viewsJsonl } from "./views.ts";

mkdirSync(new URL("../../public/fixtures/sessions/", import.meta.url), { recursive: true });
for (const [name, text] of [
  ["demo", demoJsonl()],
  ["primitives", primitivesJsonl()],
  ["diagrams", diagramsJsonl()],
  ["views", viewsJsonl()],
  ["drawing", drawingJsonl()],
  ["sources", sourcesJsonl()],
  ...sessionFixtures().map(([id, log]) => [`sessions/${id}`, log] as const),
] as const) {
  const target = new URL(`../../public/fixtures/${name}.events.jsonl`, import.meta.url);
  writeFileSync(target, text);
  console.info(`wrote ${target.pathname}`);
}
