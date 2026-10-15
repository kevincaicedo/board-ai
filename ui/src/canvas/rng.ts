// Seeded randomness for the stroke engine. The canvas never calls Math.random: every wobble comes
// from a generator seeded by the element id, so a replay draws identical strokes (LLD §4).

export type Random = () => number;

/** mulberry32, the generator the design prototype uses: 32-bit state, uniform in [0, 1). */
export function mulberry32(seed: number): Random {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** FNV-1a over UTF-16 code units: a stable 32-bit seed for an element id. */
export function hashSeed(text: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

export function randomFor(id: string): Random {
  return mulberry32(hashSeed(id));
}
