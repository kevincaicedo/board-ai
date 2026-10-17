//! The limits of staged diagrams and drill-down (D-187, D-188; LLD §5). Each says what happens
//! when it is crossed.

/// A `<diagram>` or `<detail>` block's written text. Past it the block's later tags are dropped
/// and counted, and it is staged with what came before; it still ends at its closing tag.
pub const DIAGRAM_BLOCK_MAX_BYTES: usize = 16 * 1024;

/// Text inside a block that isn't a tag: past this much in one stretch, the talker forgot the
/// closing tag and is speaking again, so the block ends there and the text is speech.
pub const DIAGRAM_GAP_TEXT_MAX_BYTES: usize = 240;

/// Parts of one diagram, connectors included. Later parts are dropped and counted, and the
/// talker is told.
pub const DIAGRAM_PARTS_MAX: usize = 200;

/// Containers nested in one diagram, the outermost counting one. A container deeper than this
/// is dropped and counted; what it held goes into the deepest container kept.
pub const DIAGRAM_NESTING_MAX: usize = 6;

/// Categories one diagram colours its parts by: the canvas's palette (slate, violet, blue,
/// green). A part naming one more keeps no colour, and the drop is counted.
pub const DIAGRAM_CATEGORIES_MAX: usize = 4;

/// A note's text in a diagram; a longer note is cut and ends with "…". It wraps onto at most
/// [`DIAGRAM_NOTE_LINES_MAX`] lines, the last ending with "…" when it doesn't fit.
pub const DIAGRAM_NOTE_MAX_BYTES: usize = 200;
pub const DIAGRAM_NOTE_LINES_MAX: usize = 6;

/// Fields of one record; later fields are dropped.
pub const RECORD_FIELDS_MAX: usize = 16;

/// Staged diagrams on one layer; another is dropped and the talker told (turn the page).
pub const DIAGRAMS_PER_LAYER_MAX: usize = 8;

/// A reveal's budget: drawn at a hand's pace while the narration explains it, until the next
/// drawing's word or the end of what is said, and never longer than this; past it the reveal
/// is drawn faster.
pub const REVEAL_BUDGET_MAX_MS: u32 = 20_000;

/// A reveal with nothing more said to time it by (the parts a reply's diagrams still hold when
/// it ends, D-237, D-250) is drawn at this pace per part, up to [`REVEAL_BUDGET_MAX_MS`]: 40
/// parts in 8 s, a hand filling in what the explanation didn't name, rather than at once.
pub const LEFTOVER_REVEAL_MS_PER_PART: u32 = 200;

/// The span a staged diagram may add to a page that already has drawing, in world units: two
/// boards' worth at 1440 × 900. Past it the diagram goes on a fresh page (D-177); on a detour or
/// detail layer, or with no page left, it is laid out anyway and the camera shows it smaller,
/// never cropped.
pub const STAGED_PAGE_WIDTH_UNITS: u32 = 2_800;
pub const STAGED_PAGE_HEIGHT_UNITS: u32 = 1_600;

/// A reply that revealed this many parts ends by showing its whole diagram once.
pub const DIAGRAM_OVERVIEW_PARTS_MIN: usize = 12;

/// Words of "more detail on …" that name what to drill into; longer is a question, kept for the
/// talker.
pub const DETAIL_TARGET_WORDS_MAX: usize = 6;
