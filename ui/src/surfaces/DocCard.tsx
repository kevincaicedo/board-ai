// The Doc card (artboard 14b): the document's title and the checked citation's chip, the figure
// being discussed (its caption, never its image: it is redrawn on the board, D-260), the passage
// with the sentence being explained highlighted in sync with the voice, and who read it. The
// highlight eases between sentences on session time, written from the frame loop.

import { Fragment } from "preact";
import { useMemo, useRef } from "preact/hooks";
import { numberWriter, plain, useFrame, useRuntime } from "../app/hooks.ts";
import type { Highlight } from "../app/session.ts";
import { round2 } from "../canvas/math.ts";
import { type DocSurface, docSentences, figureLines, writeSentenceStrengths } from "./doc.ts";

export function DocCard({
  surface,
  highlights,
}: {
  surface: DocSurface;
  highlights: readonly Highlight[];
}) {
  const runtime = useRuntime();
  const tokens = runtime.tokens;
  const sentenceRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const write = useMemo(numberWriter, []);
  const sentences = useMemo(() => docSentences(surface), [surface]);
  const strengths = useMemo(() => new Float64Array(sentences.length), [sentences]);
  const figure = figureLines(surface);
  const moves = useMemo(
    () => highlights.map((highlight) => ({ index: highlight.line, at: highlight.at })),
    [highlights],
  );

  // Every frame: the strengths go into one array, and a sentence's style is written only when
  // its strength changed.
  useFrame((at) => {
    writeSentenceStrengths(moves, at, tokens.durationStandardMs, strengths);
    const elements = sentenceRefs.current;
    for (let index = 0; index < strengths.length; index += 1) {
      write(elements[index] ?? null, "--doc-mark", round2(strengths[index] ?? 0), plain);
    }
  });

  return (
    <div class="doc-card">
      <div class="doc-head">
        <span class="card-title doc-title">{surface.title}</span>
        <span class="doc-chip">{surface.chip}</span>
      </div>
      {figure === null ? null : (
        <div class="doc-figure" role="note" aria-label={`${figure.label}: ${figure.caption}`}>
          <span class="doc-figure-label">{figure.label}</span>
          <span class="doc-figure-caption">{figure.caption}</span>
          {figure.status === null ? null : (
            <span class="doc-figure-status doc-figure-unread">{figure.status}</span>
          )}
        </div>
      )}
      {sentences.length === 0 ? null : (
        <p class="doc-passage">
          {sentences.map((sentence) => (
            <Fragment key={sentence.key}>
              {sentence.index === 0 ? null : " "}
              <span
                class="doc-sentence"
                ref={(element) => {
                  sentenceRefs.current[sentence.index] = element;
                }}
              >
                {sentence.text}
              </span>
            </Fragment>
          ))}
        </p>
      )}
      {surface.note === "" ? null : <span class="doc-note">{surface.note}</span>}
    </div>
  );
}
