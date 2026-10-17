// Pieces shared by the draw ops and a staged diagram's parts (LLD §3.7): edge paths, cells and
// the enums they name, each checked as the core sends it.

import { elementId, fields, nullable, point, rect, text } from "./fields.ts";
import type { CellBox } from "./generated/CellBox.ts";
import type { EdgePath } from "./generated/EdgePath.ts";
import type { LineStyle } from "./generated/LineStyle.ts";
import type { NodeShape } from "./generated/NodeShape.ts";

export function edgePath(value: unknown): EdgePath | null {
  const f = fields(value);
  const start = point(f?.start);
  const end = point(f?.end);
  const control = nullable(f?.control, point);
  return start === null || end === null || control === undefined ? null : { start, control, end };
}

export const NODE_SHAPES: readonly NodeShape[] = [
  "Service",
  "Db",
  "Queue",
  "Actor",
  "Group",
  "Note",
  "Circle",
  "Pill",
];
export const LINE_STYLES: readonly LineStyle[] = ["Solid", "Dashed"];

export function cellBox(value: unknown): CellBox | null {
  const f = fields(value);
  const id = elementId(f?.id);
  const r = rect(f?.rect);
  const content = text(f?.value);
  const index = nullable(f?.index, text);
  return id === null || r === null || content === null || index === undefined
    ? null
    : { id, rect: r, value: content, index };
}
