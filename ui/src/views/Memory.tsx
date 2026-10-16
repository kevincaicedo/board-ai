// Memory (Brain · memories and notes): two frosted panels. "About you" holds what BoardAI keeps
// about how you learn and about your systems; "Notes" holds the latest notes with their sources,
// and the last session's digest at the bottom.

import type { MemoryRow } from "../protocol/generated/MemoryRow.ts";
import type { View } from "../protocol/generated/View.ts";
import { dayLabel, digestCounts, joined, shortTitle } from "./labels.ts";
import { MemoryGlyph } from "./parts.tsx";
import { Sketch } from "./Sketch.tsx";

const THUMB_WIDTH = 76;
const THUMB_HEIGHT = 56;

function MemoryRows({ rows }: { rows: readonly MemoryRow[] }) {
  return (
    <div class="panel-rows">
      {rows.map((row) => (
        <div key={row.id} class="memory-row">
          <MemoryGlyph tone="ink" />
          <span class="memory-text">{row.text}</span>
        </div>
      ))}
    </div>
  );
}

export function MemoryView({
  view,
  today,
}: {
  view: Extract<View, { kind: "Memory" }>;
  today: string;
}) {
  const latest = view.latest;
  const nothing = view.about_you.length === 0 && view.systems.length === 0;
  return (
    <div class="memory-panels">
      <section class="panel panel-memory" aria-label="Memory">
        <div class="panel-head">
          <span class="panel-label">Memory</span>
          <h2 class="panel-title panel-title-memory">About you</h2>
        </div>
        {view.about_you.length > 0 ? (
          <div class="panel-section" data-stagger="0">
            <span class="panel-section-title">How you like to learn</span>
            <MemoryRows rows={view.about_you} />
          </div>
        ) : null}
        {view.systems.length > 0 ? (
          <div class="panel-section" data-stagger="1">
            <span class="panel-section-title">Your systems</span>
            <MemoryRows rows={view.systems} />
          </div>
        ) : null}
        {nothing ? (
          <p class="panel-empty">Nothing kept yet. Tell BoardAI how you like to learn.</p>
        ) : null}
        <span class="panel-foot">
          Plain Markdown in your vault. Say “forget that” to undo the last one.
        </span>
      </section>
      <section class="panel panel-memory panel-notes" aria-label="Notes">
        <div class="panel-head">
          <span class="panel-label">Notes</span>
          <h2 class="panel-title panel-title-memory">Recent</h2>
        </div>
        {view.notes.length === 0 ? (
          <p class="panel-empty">No notes yet. Say “note that…” while we talk.</p>
        ) : (
          <div class="panel-rows panel-rows-notes" data-stagger="0">
            {view.notes.map((note) => {
              const source = joined(
                note.from === null ? "" : shortTitle(note.from.title),
                dayLabel(note.date, today),
              );
              return (
                <div key={note.id} class="note-card">
                  <span class="note-text">{note.text}</span>
                  {note.links.length > 0 || source !== "" ? (
                    <div class="note-chips">
                      {note.links.map((link) => (
                        <span key={link} class="link-chip link-chip-code">
                          {link}
                        </span>
                      ))}
                      {source === "" ? null : <span class="link-chip">{source}</span>}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        )}
        {latest === null ? null : (
          <div class="panel-section panel-digest" data-stagger="1">
            <span class="panel-section-title">
              {joined("Session digest", dayLabel(latest.session.date, today))}
            </span>
            <div class="digest-card">
              <div class="digest-thumb">
                <Sketch shapes={latest.session.cover} width={THUMB_WIDTH} height={THUMB_HEIGHT} />
              </div>
              <div class="digest-text">
                <span class="digest-title">{latest.session.title}</span>
                <span class="row-meta">{digestCounts(latest)}</span>
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
