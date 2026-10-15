//! The core's named limits (LLD §5). Each one says what happens when it is crossed.

/// Longest element id the talker may use; a longer id drops the tag (counted).
pub const ELEMENT_ID_MAX_BYTES: usize = 64;

/// Longest provider tool-call id; a longer one makes the call invalid.
pub const TOOL_CALL_ID_MAX_BYTES: usize = 128;

/// Largest tool input JSON; larger input is rejected with `InvalidToolInput::TooLarge`.
pub const TOOL_INPUT_MAX_BYTES: usize = 64 * 1024;

/// Longest single string argument of a tool call; longer is rejected as malformed.
pub const TOOL_ARGUMENT_MAX_BYTES: usize = 16 * 1024;

/// Longest narration tag; a longer tag is dropped and counted, and speech continues.
pub const NARRATION_TAG_MAX_BYTES: usize = 512;

/// Longest clause sent to text-to-speech; the clause is cut at this size.
pub const CLAUSE_MAX_BYTES: usize = 300;

/// A comma ends a clause once the clause is at least this long, so first audio comes early.
pub const CLAUSE_COMMA_MIN_BYTES: usize = 40;

/// Longest utterance; the turn ends when it is reached.
pub const UTTERANCE_MAX_MS: u64 = 30_000;

/// Ops per talker response; the rest are dropped and the talker is told in the next turn.
pub const OPS_PER_RESPONSE_MAX: usize = 200;

/// Open detours; a fourth gets a spoken "let's finish this one first".
pub const DETOUR_DEPTH_MAX: usize = 3;

/// Silence that ends a turn when the sentence sounds finished.
pub const END_OF_TURN_SILENCE_MS: u64 = 600;

/// How long to wait when the turn ends on a continuation word.
pub const END_OF_TURN_GRACE_MS: u64 = 1_500;

/// Barge-in: undecided speech becomes an interruption after this long.
pub const BARGE_IN_DECISION_MS: u64 = 900;

/// Barge-in: this much voice makes it an interruption.
pub const BARGE_IN_VOICE_MS: u32 = 700;

/// Barge-in: this many words make it an interruption.
pub const BARGE_IN_WORDS_MIN: usize = 4;

/// Barge-in: a pointer target set this recently makes it an interruption.
pub const BARGE_IN_POINTER_WINDOW_MS: u64 = 1_500;

/// Op budget bounds: the UI finishes a stroke within this window (LLD §2.4).
pub const OP_BUDGET_MIN_MS: u32 = 150;
pub const OP_BUDGET_MAX_MS: u32 = 2_500;

/// Ops with no speech between them start at least this far apart.
pub const OP_SPACING_MIN_MS: u32 = 120;
