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

// ---- Conversation (LLD §2.2) ----

/// Barge-in rule (a) looks for an interruption cue in this many leading words.
pub const BARGE_IN_CUE_WORDS: usize = 3;

/// A final transcript this soon after a backchannel decision can still carry an interruption cue
/// ("wait" said too fast for a partial); it then stops the voice. Later finals are ignored.
pub const BACKCHANNEL_LATE_FINAL_MS: u64 = 600;

/// How long the core waits for `VoiceStopped` after `StopVoice`. When it doesn't come, the last
/// reported playback position defines the heard prefix (it is never ahead of the audio).
pub const VOICE_STOP_TIMEOUT_MS: u64 = 500;

/// Longest utterance transcript kept; longer text is cut at a char boundary.
pub const UTTERANCE_MAX_BYTES: usize = 4 * 1024;

// ---- Narration (LLD §2.3) ----

/// Longest response text the core accepts from one talker stream. Past it the stream is
/// cancelled and treated like `MaxTokens`: the current sentence finishes.
pub const RESPONSE_TEXT_MAX_BYTES: usize = 32 * 1024;

/// After an oversized or malformed tag, the parser skips at most this much looking for its `>`,
/// then returns to speech.
pub const NARRATION_SKIP_MAX_BYTES: usize = 2 * 1024;

/// Longest node, edge or annotation label; longer labels are cut and end with "…".
pub const LABEL_MAX_BYTES: usize = 80;

/// Longest detour title or plan step; longer ones are cut.
pub const TITLE_MAX_BYTES: usize = 60;

/// Steps in a plan; extra steps are dropped.
pub const PLAN_STEPS_MAX: usize = 12;

/// Copies drawn for `stack`; a larger stack is clamped.
pub const NODE_STACK_MAX: u8 = 9;

/// Options or criteria of a compare card; extra ones are dropped.
pub const COMPARE_AXIS_MAX: usize = 6;

/// Hops in a `trace` path; a longer path drops the tag.
pub const TRACE_HOPS_MAX: usize = 16;

// ---- Board, surfaces and talker context (LLD §2.5, §2.6) ----

/// Elements on one layer; an op past it is dropped and counted.
pub const ELEMENTS_PER_LAYER_MAX: usize = 500;

/// Layers per session, main included (`LayerId` is a `u8`); a detour past it is refused out
/// loud, like a fourth nested detour.
pub const LAYERS_PER_SESSION_MAX: usize = 256;

/// Cells layout tries along one direction before falling back to a new row under everything.
pub const LAYOUT_NUDGE_CELLS_MAX: i32 = 64;

/// Surfaces beside the canvas (PRODUCT §7.2); opening another dismisses the oldest.
pub const SURFACES_BESIDE_CANVAS_MAX: usize = 2;

/// Files whose read lines the core keeps for code cards; the least recently read is forgotten,
/// and a code card for it is refused until it is read again.
pub const READ_FILES_MAX: usize = 64;

/// Lines kept per read file; lines past it are not kept.
pub const READ_FILE_LINES_MAX: usize = 4_000;

/// Lines on one code card; a longer range is cut.
pub const CODE_CARD_LINES_MAX: usize = 60;

/// Board summary sent to the talker; longer summaries end with "…".
pub const BOARD_SUMMARY_MAX_BYTES: usize = 4 * 1024;

/// How much of the heard text the interruption note quotes (`…checks Re`).
pub const HEARD_NOTE_TAIL_BYTES: usize = 120;

/// Notices kept for the talker's next request (dropped ops, refused code cards); older ones are
/// dropped first.
pub const TALKER_NOTICES_MAX: usize = 8;

// ---- Tools and turns (LLD §3.3, §3.5) ----

/// Tool calls in one talker response; extra calls get an error result.
pub const TOOL_CALLS_PER_RESPONSE_MAX: usize = 16;

/// Tool rounds in one turn. Past it the calls get error results, the turn ends, and the user
/// hears a failure line.
pub const TOOL_ROUNDS_PER_TURN_MAX: usize = 8;

/// Longest branch slug for a change (`boardai/<slug>`); longer slugs are cut at a word.
pub const BRANCH_SLUG_MAX_BYTES: usize = 40;

/// Harness results waiting to be reported; older ones are reported together in one message.
pub const HARNESS_REPORTS_MAX: usize = 8;

// ---- Speech-to-stroke timing (LLD §2.4) ----

/// Speech rate used to estimate when a not-yet-synthesized clause will play (about 15
/// characters a second), for op budgets only; firing always waits for real playback.
pub const ESTIMATED_SPEECH_MS_PER_BYTE: u32 = 65;

/// The orb stays in its drawing state this long after the last stroke's budget ends, so the nib
/// doesn't flicker between ops.
pub const ORB_DRAWING_LINGER_MS: u64 = 800;
