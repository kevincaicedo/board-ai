// The surfaces beside the canvas (PRODUCT §7.2, Components 10b): the detour card that holds a
// detour's board layer, the code card with its moving highlight, compare and recap. Frosted
// glass, no borders; time-driven styles are written from the frame loop.

import { useLayoutEffect, useMemo, useRef } from "preact/hooks";
import { styleWriter, useFrame, useRuntime } from "../app/hooks.ts";
import type { CellView, ColumnItem, Highlight } from "../app/session.ts";
import { clamp, easeOut, lerp, ramp, round1, round2 } from "../canvas/math.ts";
import { revealLength } from "../canvas/stroke.ts";
import type { Surface } from "../protocol/generated/Surface.ts";
import { lineRange, tokenizeLine } from "./code.ts";
import { markStrokes } from "./marks.ts";

export function DetourCard({ item }: { item: Extract<ColumnItem, { kind: "detour" }> }) {
  const runtime = useRuntime();
  const bodyRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    runtime.attachLayerHost(item.layer, bodyRef.current);
    return () => runtime.attachLayerHost(item.layer, null);
  }, [runtime, item.layer]);
  return (
    <>
      <div class="card-head">
        <svg class="detour-glyph" aria-hidden="true" width="14" height="14" viewBox="0 0 14 14">
          <path d="M3 1.5v5a4 4 0 0 0 4 4h4" />
          <circle cx="11.5" cy="10.5" r="1.4" />
        </svg>
        <span class="card-title">{item.title}</span>
        <span class="card-sub">Detour</span>
      </div>
      <div class="detour-body" ref={bodyRef} />
    </>
  );
}

/** The highlight's eased line index: it glides from the previous line to the new one. */
function highlightIndex(
  highlights: readonly Highlight[],
  firstLine: number,
  at: number,
  durationMs: number,
): number | null {
  const latest = highlights[highlights.length - 1];
  if (latest === undefined) {
    return null;
  }
  const target = latest.line - firstLine;
  const before = highlights[highlights.length - 2];
  if (before === undefined) {
    return target;
  }
  return lerp(
    before.line - firstLine,
    target,
    easeOut(ramp(at, latest.at, latest.at + durationMs)),
  );
}

export function CodeCard({
  surface,
  highlights,
}: {
  surface: Extract<Surface, { kind: "Code" }>;
  highlights: readonly Highlight[];
}) {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const bandRef = useRef<HTMLDivElement>(null);
  const lineRefs = useRef<(HTMLDivElement | null)[]>([]);
  const write = useMemo(styleWriter, []);
  const lines = useMemo(() => surface.lines.map(tokenizeLine), [surface.lines]);

  useFrame((at) => {
    const index = highlightIndex(highlights, surface.first_line, at, tokens.durationStandardMs);
    const first = highlights[0];
    const band =
      first === undefined ? 0 : easeOut(ramp(at, first.at, first.at + tokens.durationBandInMs));
    write(bandRef.current, "opacity", String(round2(band)));
    if (index !== null) {
      write(bandRef.current, "transform", `translateY(${round1(tokens.codeLine * index)}px)`);
    }
    const base = lerp(1, 0.42, band);
    lineRefs.current.forEach((line, lineIndex) => {
      const near = index === null ? 0 : clamp(1 - Math.abs(lineIndex - index) * 1.1);
      write(line, "opacity", String(round2(base + (1 - base) * near)));
    });
  });

  return (
    <>
      <div class="card-head">
        <span class="card-path">{surface.path}</span>
        <span class="card-sub">· {lineRange(surface.first_line, surface.lines.length)}</span>
      </div>
      <div class="code-band" ref={bandRef} aria-hidden="true" />
      <div class="code-lines">
        {lines.map((tokensOfLine, index) => {
          const number = surface.first_line + index;
          return (
            <div
              key={number}
              class="code-line"
              ref={(element) => {
                lineRefs.current[index] = element;
              }}
              onPointerEnter={() =>
                runtime.pointer({ kind: "CodeLine", surface: surface.id, line: number })
              }
              onPointerLeave={() => runtime.pointer(null)}
            >
              <span class="code-gutter">{number}</span>
              <span class="code-text">
                {tokensOfLine.map((token, tokenIndex) =>
                  token.kind === "plain" ? (
                    token.text
                  ) : (
                    <span key={tokenIndex} class={`code-${token.kind}`}>
                      {token.text}
                    </span>
                  ),
                )}
              </span>
            </div>
          );
        })}
      </div>
    </>
  );
}

function CompareCell({
  surfaceId,
  row,
  col,
  cell,
}: {
  surfaceId: number;
  row: number;
  col: number;
  cell: CellView | undefined;
}) {
  const runtime = useRuntime();
  const { durationCellMarkMs, durationCellTextMs } = runtime.tokens;
  const pathsRef = useRef<(SVGPathElement | null)[]>([]);
  const textRef = useRef<HTMLSpanElement>(null);
  const write = useMemo(styleWriter, []);
  const mark = cell?.mark;
  const strokes = useMemo(
    () =>
      mark === undefined || mark.kind === "Text"
        ? []
        : markStrokes(mark.kind, `${surfaceId}:${row}:${col}`),
    [mark, surfaceId, row, col],
  );
  useFrame((at) => {
    if (cell === undefined) {
      return;
    }
    if (cell.mark.kind === "Text") {
      const f = clamp((at - cell.at) / durationCellTextMs);
      const letters = Array.from(cell.mark.text);
      const shown = f >= 1 ? letters.length : Math.floor(f * letters.length);
      if (textRef.current !== null && textRef.current.dataset.shown !== String(shown)) {
        textRef.current.dataset.shown = String(shown);
        textRef.current.textContent = letters.slice(0, shown).join("");
      }
      return;
    }
    const per = durationCellMarkMs / Math.max(1, strokes.length);
    strokes.forEach((stroke, index) => {
      const f = ramp(at, cell.at + index * per, cell.at + (index + 1) * per);
      write(
        pathsRef.current[index] ?? null,
        "stroke-dashoffset",
        String(round1(stroke.length - revealLength(stroke, f))),
      );
    });
  });
  if (cell === undefined) {
    return <span class="compare-cell" />;
  }
  if (cell.mark.kind === "Text") {
    return (
      <span class="compare-cell compare-text" role="img" aria-label={cell.mark.text}>
        <span ref={textRef} />
      </span>
    );
  }
  const label = cell.mark.kind === "Yes" ? "yes" : cell.mark.kind === "No" ? "no" : "depends";
  return (
    <span class="compare-cell">
      <svg role="img" aria-label={label} width="28" height="26" viewBox="0 0 28 26">
        {strokes.map((stroke, index) => (
          <path
            key={stroke.path}
            ref={(element) => {
              pathsRef.current[index] = element;
            }}
            class={`stroke tone-${stroke.tone}`}
            d={stroke.path}
            stroke-width={stroke.width}
            stroke-dasharray={`${round1(stroke.length)} ${round1(stroke.length + 20)}`}
            stroke-dashoffset={round1(stroke.length)}
          />
        ))}
      </svg>
    </span>
  );
}

export function CompareCard({
  surface,
  cells,
}: {
  surface: Extract<Surface, { kind: "Compare" }>;
  cells: ReadonlyMap<string, CellView>;
}) {
  const columns = `minmax(0, 1fr) repeat(${surface.options.length}, minmax(112px, max-content))`;
  return (
    <>
      <div class="card-head">
        <span class="card-title">{surface.options.join(" vs ")}</span>
      </div>
      <div class="compare-grid" style={{ gridTemplateColumns: columns }}>
        <span />
        {surface.options.map((option) => (
          <span key={option} class="compare-option">
            {option}
          </span>
        ))}
        {surface.criteria.map((criterion, col) => [
          <span key={`label-${criterion}`} class="compare-criterion">
            {criterion}
          </span>,
          ...surface.options.map((option, row) => (
            <CompareCell
              key={`${option}-${criterion}`}
              surfaceId={surface.id}
              row={row}
              col={col}
              cell={cells.get(`${row}:${col}`)}
            />
          )),
        ])}
      </div>
    </>
  );
}

export function RecapCard({ surface }: { surface: Extract<Surface, { kind: "Recap" }> }) {
  return (
    <>
      <div class="card-head">
        <span class="card-title">Recap</span>
      </div>
      <ul class="recap-list">
        {surface.bullets.map((bullet) => (
          <li key={bullet}>{bullet}</li>
        ))}
      </ul>
    </>
  );
}
