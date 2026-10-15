// Word timings for authored fixtures, ported from the design's Speech.say: a word takes 100 ms
// plus 33 ms per letter, with pauses after commas, sentence ends and dashes. A fixture uses it to
// emit captions word by word, as the runtime would while the voice plays.

import type { Speaker } from "../protocol/generated/Speaker.ts";

export interface Utterance {
  readonly who: Speaker;
  readonly words: readonly string[];
  /** Session ms at which each word starts. */
  readonly wordAt: readonly number[];
  readonly startAt: number;
  /** When the voice stops: the natural end, or the cut. */
  readonly endAt: number;
  readonly cut: boolean;
}

export interface SayOptions {
  /** Cut the voice this long after the last word starts (an interruption). */
  readonly cutAfterMs?: number;
  /** Pause after the final sentence end instead of the usual 260 ms. */
  readonly endPauseMs?: number;
}

export function say(
  who: Speaker,
  startAt: number,
  text: string,
  options: SayOptions = {},
): Utterance {
  const words = text.split(" ");
  const wordAt: number[] = [];
  let time = startAt;
  words.forEach((word, index) => {
    wordAt.push(Math.round(time));
    let duration = 100 + 33 * word.replace(/[^A-Za-z0-9]/g, "").length;
    if (/[,:]$/.test(word)) {
      duration += 150;
    }
    if (/[.?]$/.test(word)) {
      duration +=
        index === words.length - 1 && options.endPauseMs !== undefined ? options.endPauseMs : 260;
    }
    if (/—$/.test(word)) {
      duration += 200;
    }
    time += duration;
  });
  const last = wordAt[wordAt.length - 1] ?? startAt;
  const cut = options.cutAfterMs !== undefined;
  return {
    who,
    words,
    wordAt,
    startAt,
    endAt: Math.round(cut ? last + (options.cutAfterMs ?? 0) : time),
    cut,
  };
}
