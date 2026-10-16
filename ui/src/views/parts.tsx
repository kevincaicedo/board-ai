// Small pieces the views share, drawn as on the canvas: the round play and mute controls, the
// thin progress track, the memory glyph and a task's empty circle.

export function PlayGlyph() {
  return (
    <svg class="glyph-play" aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
      <path d="M4.5 2.8v10.4c0 .6.6.9 1.1.6l8-5.2c.5-.3.5-.9 0-1.2l-8-5.2c-.5-.3-1.1 0-1.1.6z" />
    </svg>
  );
}

export function MuteGlyph({ muted }: { muted: boolean }) {
  return (
    <svg class="glyph-sound" aria-hidden="true" width="20" height="20" viewBox="0 0 20 20">
      <path class="glyph-sound-body" d="M3 7.5h2.6L9.5 4v12l-3.9-3.5H3z" />
      {muted ? (
        <path d="M13 7.5l4 5M17 7.5l-4 5" />
      ) : (
        <path d="M12.8 7.2a4 4 0 0 1 0 5.6M15 5.2a7 7 0 0 1 0 9.6" />
      )}
    </svg>
  );
}

export function MemoryGlyph({ tone }: { tone: "signal" | "ink" }) {
  return (
    <svg
      class={`glyph-memory glyph-memory-${tone}`}
      aria-hidden="true"
      width="16"
      height="16"
      viewBox="0 0 16 16"
    >
      <circle cx="8" cy="8" r="6" />
      <circle class="glyph-memory-dot" cx="8" cy="8" r="2" />
    </svg>
  );
}

export function TaskCircle() {
  return <span class="task-circle" aria-hidden="true" />;
}

export function CloseGlyph() {
  return (
    <svg class="glyph-close" aria-hidden="true" width="16" height="16" viewBox="0 0 16 16">
      <path d="M4 4l8 8M12 4l-8 8" />
    </svg>
  );
}

/** The thin track with a signal fill (library cards, the replay card). */
export function ProgressTrack({
  fraction,
  label,
  variant,
}: {
  fraction: number;
  label: string;
  variant: "on-cover" | "on-glass";
}) {
  const percent = Math.round(Math.min(1, Math.max(0, fraction)) * 100);
  return (
    <div class={`track track-${variant}`} role="img" aria-label={label}>
      <div class="track-fill" style={{ width: `${percent}%` }} />
    </div>
  );
}
