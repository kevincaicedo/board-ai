// The bounds of drawing together (13 · You draw), mirrored from the core's limits module
// (crates/core/src/limits.rs, "You draw"). The UI never sends past them, and the parser refuses
// an echo past them.

/** Points in one pen stroke: the core's `USER_STROKE_POINTS_MAX`. Sampling decimates to fit. */
export const USER_STROKE_POINTS_MAX = 512;
/** Pen strokes in one shape: the core's `USER_SHAPE_STROKES_MAX`. */
export const USER_SHAPE_STROKES_MAX = 32;
/**
 * Your shapes kept per layer: the core's `USER_SHAPES_PER_LAYER_MAX`. A new id past it is dropped
 * and counted (the core never sends one).
 */
export const USER_SHAPES_PER_LAYER_MAX = 200;
/** The core refuses a shape smaller than this (a click); the UI doesn't send one. */
export const USER_SHAPE_MIN_UNITS = 6;
/** Screen pixels between two sampled pen points; closer moves are skipped. */
export const PEN_SAMPLE_MIN_PX = 3;
