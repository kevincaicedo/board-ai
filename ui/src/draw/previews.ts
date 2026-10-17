// The previews of shapes sent to the core and not yet echoed (13 · You draw): each stays until the
// core's copy arrives, or the hold runs out because the core refused it. Pure functions over an
// immutable list, so the surface keeps it in component state and node tests it.

/** Previews waiting for the core's echo; past this the oldest goes at once. */
export const PENDING_PREVIEWS_MAX = 8;

export interface Preview {
  readonly serial: number;
  /** The gesture's path, in screen pixels. */
  readonly d: string;
  /** The model's count that means this shape is on the board. */
  readonly awaits: number;
  readonly sentAt: number;
}

/** A shape was just sent; `counted` is the model's count now. */
export function sent(
  previews: readonly Preview[],
  serial: number,
  d: string,
  counted: number,
  now: number,
): readonly Preview[] {
  return [...previews, { serial, d, awaits: counted + 1, sentAt: now }].slice(
    -PENDING_PREVIEWS_MAX,
  );
}

/** The model's count moved: previews it covers go. */
export function echoed(previews: readonly Preview[], counted: number): readonly Preview[] {
  const kept = previews.filter((preview) => preview.awaits > counted);
  return kept.length === previews.length ? previews : kept;
}

/** The hold ran out for the previews sent at least `holdMs` ago. */
export function expired(previews: readonly Preview[], now: number, holdMs: number) {
  const kept = previews.filter((preview) => now - preview.sentAt < holdMs);
  return kept.length === previews.length ? previews : kept;
}

/** How long until the oldest preview's hold runs out; `null` with none waiting. */
export function nextExpiry(previews: readonly Preview[], now: number, holdMs: number) {
  const oldest = previews[0];
  return oldest === undefined ? null : Math.max(0, oldest.sentAt + holdMs - now);
}
