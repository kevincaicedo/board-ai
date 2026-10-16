// The library (8 · Library): past sessions as large rounded cards, newest first. Each card has
// soft cover art from its board, a play button that replays it here, a mute toggle (BoardAI's
// voice isn't stored yet, so it only changes its glyph) and how far it was watched.

import { useState } from "preact/hooks";
import { useRuntime } from "../app/hooks.ts";
import type { SessionCard } from "../protocol/generated/SessionCard.ts";
import type { View } from "../protocol/generated/View.ts";
import { Cover } from "./Cover.tsx";
import { libraryHint, sessionMeta } from "./labels.ts";
import { MuteGlyph, PlayGlyph, ProgressTrack } from "./parts.tsx";

/** The cover's drawing size; the card crops it to its own width. */
const COVER_WIDTH = 400;
const COVER_HEIGHT = 300;

function LibraryCard({ card, today }: { card: SessionCard; today: string }) {
  const runtime = useRuntime();
  const [muted, setMuted] = useState(true);
  const played = runtime.replays.progress.value.get(card.id) ?? 0;
  return (
    <article class="library-card">
      <div class="library-cover">
        <Cover shapes={card.cover} seed={card.id} width={COVER_WIDTH} height={COVER_HEIGHT} />
        <div class="library-cover-fade" aria-hidden="true" />
        <div class="library-controls">
          <button
            type="button"
            class="card-control"
            aria-label={`Play ${card.title}`}
            onClick={() => runtime.replays.open(card, "library")}
          >
            <PlayGlyph />
          </button>
          <button
            type="button"
            class="card-control"
            aria-label={muted ? "Unmute" : "Mute"}
            aria-pressed={muted}
            onClick={() => setMuted(!muted)}
          >
            <MuteGlyph muted={muted} />
          </button>
          <ProgressTrack
            fraction={played}
            label={`${Math.round(played * 100)}% played`}
            variant="on-cover"
          />
        </div>
      </div>
      <div class="library-card-info">
        <span class="library-card-title">{card.title}</span>
        <span class="library-card-meta">{sessionMeta(card, today)}</span>
      </div>
    </article>
  );
}

export function LibraryView({
  view,
  today,
}: {
  view: Extract<View, { kind: "Library" }>;
  today: string;
}) {
  return (
    <section class="view-library" aria-label="Sessions">
      <div class="library-scroll">
        <header class="library-head">
          <h1 class="library-title">Sessions</h1>
          <p class="library-hint">{libraryHint(view.sessions)}</p>
        </header>
        {view.sessions.length === 0 ? (
          <p class="library-empty">No sessions yet.</p>
        ) : (
          <div class="library-grid">
            {view.sessions.map((card) => (
              <LibraryCard key={card.id} card={card} today={today} />
            ))}
          </div>
        )}
        {view.more_sessions > 0 ? (
          <p class="library-more">
            {view.more_sessions === 1 ? "1 older session" : `${view.more_sessions} older sessions`}
          </p>
        ) : null}
      </div>
    </section>
  );
}
