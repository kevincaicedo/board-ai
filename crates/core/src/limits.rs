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

/// The same for a response's first clause, which the user waits on: "A request starts on the
/// browser," (32 bytes) goes to text-to-speech while the talker is still writing the rest of the
/// sentence. Shorter than about a second of speech, the next clause might not be synthesized
/// before this one has played.
pub const CLAUSE_FIRST_COMMA_MIN_BYTES: usize = 16;

/// Longest utterance; the turn ends when it is reached.
pub const UTTERANCE_MAX_MS: u64 = 30_000;

/// Ops per talker response; the rest are dropped and the talker is told in the next turn.
pub const OPS_PER_RESPONSE_MAX: usize = 200;

/// Open detours; a fourth gets a spoken "let's finish this one first".
pub const DETOUR_DEPTH_MAX: usize = 3;

/// Silence, from the last voiced moment, that ends a turn whose final transcript sounds
/// finished (terminal punctuation, no continuation word). The runtime's detector reports
/// `VoiceEnd` after 256 ms of silence, so such a turn ends as soon as `VoiceEnd` and the final
/// are in.
pub const END_OF_TURN_FINISHED_SILENCE_MS: u64 = 250;

/// Silence, from the last voiced moment, that ends a turn whose words neither sound finished
/// nor unfinished (no punctuation, or no final yet).
pub const END_OF_TURN_SILENCE_MS: u64 = 600;

/// Silence, from the last voiced moment, that ends a turn ending on a continuation word or a
/// trailing-off mark ("so the leader…").
pub const END_OF_TURN_GRACE_MS: u64 = 1_500;

/// Barge-in: speech still undecided this long after `VoiceStart` is an interruption. It must
/// outlast the longest backchannel: measured with `say` (five voices, 150 wpm to 220 wpm, every
/// phrase of the lexicon Whisper heard as itself), the detector reports up to 960 ms of voice,
/// and its `VoiceEnd` and final come 96 ms past that on this clock (the 256 ms hangover less the
/// 160 ms `VoiceStart` lags its onset). 1,200 ms leaves ~140 ms for the final (LLD §2.2).
pub const BARGE_IN_DECISION_MS: u64 = 1_200;

/// A voice onset while speaking ducks the voice for this long. `VoiceStart` confirms speech about
/// 160 ms after its onset (the detector's 150 ms minimum, in 32 ms windows), so this leaves room
/// for a late detector thread; past it, an onset that never became speech (a cough, a click)
/// restores the gain and changes nothing else.
pub const VOICE_ONSET_DUCK_HOLD_MS: u64 = 300;

/// Barge-in: this much voice makes it an interruption. Counted live from `VoiceStart`, so it
/// includes the detector's hangover before `VoiceEnd` corrects it; the longest measured
/// backchannel reaches 1,056 ms that way (see [`BARGE_IN_DECISION_MS`]). It was 700 ms, which
/// the measured slow "all right" (768–800 ms) and "mhm" (832–960 ms) crossed (I7).
pub const BARGE_IN_VOICE_MS: u32 = 1_200;

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

/// Bullets on a recap card (PRODUCT §7.2 asks for 3 to 5); extra ones are dropped.
pub const RECAP_BULLETS_MAX: usize = 5;

/// Cells in one `cells` row or column; values and a count past it are dropped.
pub const CELLS_PER_STRIP_MAX: usize = 32;

/// Longest cell value or index label; a longer one is cut and ends with "…".
pub const CELL_VALUE_MAX_BYTES: usize = 24;

/// Longest spoken token read as a `path:line` citation; a longer token is just speech.
pub const CITATION_TOKEN_MAX_BYTES: usize = 200;

/// Longest tool call written as words that is kept out of speech (D-179): `list_dir path "."`
/// or `read_file("src/main.rs")`. Past it the call is taken to have ended and the rest is
/// speech again, so an unclosed bracket can't silence a reply; a line end ends one too.
pub const SPOKEN_CALL_MAX_BYTES: usize = 240;

/// The system prompt and the tool schemas (as JSON), together. Local models load with a small
/// context (gemma4 on Ollama: 4,096 tokens), and a request whose prompt doesn't fit loses the
/// system prompt from the front, so the model answers without drawing. At 9,150 + 5,188 bytes
/// they measured 2,466 + 1,122 gemma4 tokens, leaving about 500 for the conversation and the
/// answer; the prompt's tests fail past this. The runtime now asks a local Ollama for 16,384
/// tokens (`OLLAMA_CONTEXT_TOKENS`), so this bounds the prompt's cold prefill (~5 s at ~700
/// tokens/s) and keeps Ollama reached through its OpenAI-compatible endpoint working.
pub const PROMPT_AND_TOOLS_MAX_BYTES: usize = 14_400;

/// Source chips one reply shows; later citations in the reply show none (the talker still
/// says them).
pub const SOURCE_CHIPS_PER_REPLY_MAX: usize = 8;

// ---- Board, surfaces and talker context (LLD §2.5, §2.6) ----

/// Elements on one layer; an op past it is dropped and counted.
pub const ELEMENTS_PER_LAYER_MAX: usize = 500;

/// Layers per session, main included (`LayerId` is a `u8`); a detour past it is refused out
/// loud, like a fourth nested detour.
pub const LAYERS_PER_SESSION_MAX: usize = 256;

/// Cells layout tries along one direction before falling back to a new row under everything.
pub const LAYOUT_NUDGE_CELLS_MAX: i32 = 64;

/// Nudges a drawing placed off the grid (a tree node, a node beside a cell) takes past what is
/// in its way before it goes under everything instead.
pub const FREE_NUDGE_STEPS_MAX: i32 = 64;

/// Surfaces beside the canvas (PRODUCT §7.2); opening another dismisses the oldest.
pub const SURFACES_BESIDE_CANVAS_MAX: usize = 2;

/// Files whose read lines the core keeps for code cards; the least recently read is forgotten,
/// and a code card for it is refused until it is read again.
pub const READ_FILES_MAX: usize = 64;

/// Repo paths (with their folders) the core keeps from repo tool results, to check a module
/// map's labels against (D-180). Past it nothing more is kept and labels are no longer checked,
/// so a real name that wasn't kept is never marked as invented.
pub const REPO_NAMES_MAX: usize = 2_048;

/// Lines kept per read file; lines past it are not kept.
pub const READ_FILE_LINES_MAX: usize = 4_000;

/// Lines on one code card; a longer range is cut.
pub const CODE_CARD_LINES_MAX: usize = 60;

/// The idle suggestion under the orb; a longer one is cut and ends with "…".
pub const SUGGESTION_MAX_BYTES: usize = 120;

/// The question a detour card shows under its title; a longer one is cut and ends with "…".
pub const DETOUR_QUESTION_MAX_BYTES: usize = 80;

/// Space the camera keeps around a layer's drawing (and around a focused element), in world
/// units, so the newest stroke never touches the edge of the view.
pub const CAMERA_MARGIN_UNITS: i32 = 60;

/// How far the drawing must grow past the current frame before the camera follows, in world
/// units. Smaller growth (an annotation, a label) stays within the margin and moves nothing, so
/// the view doesn't twitch with every mark; a new node always moves it.
pub const CAMERA_REFRAME_STEP_UNITS: i32 = 40;

/// Pages one session may use, the first included (D-177). A page turn past it is refused: the
/// board stays as it is, the user hears so when they asked, and the talker gets a notice. Every
/// page is kept whole for the session's export, so this bounds that memory (each page holds at
/// most `ELEMENTS_PER_LAYER_MAX` elements).
pub const PAGES_PER_SESSION_MAX: u16 = 32;

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

/// Times the core asks the talker once more in one turn, when the reply drew nothing though a
/// drawing was asked for, or wrote a repo read instead of calling it (D-178). Past it the reply
/// ends as it is: one more ask that still misses means the model can't, and a loop would hold
/// the floor.
pub const NUDGES_PER_TURN_MAX: usize = 1;

/// Tool rounds in one turn. Past it the calls get error results, the turn ends, and the user
/// hears a failure line.
pub const TOOL_ROUNDS_PER_TURN_MAX: usize = 8;

/// Longest branch slug for a change (`boardai/<slug>`); longer slugs are cut at a word.
pub const BRANCH_SLUG_MAX_BYTES: usize = 40;

/// Longest `propose_change` instruction, whitespace collapsed. BoardAI reads it to the user word
/// for word before asking for the yes, and exactly it runs (I9), so it must be one short spoken
/// sentence; with the branch the question stays under the 300-byte clause cap. A longer one is
/// refused with an error result asking for a shorter one.
pub const CHANGE_INSTRUCTION_MAX_BYTES: usize = 200;

/// Harness results waiting to be reported; older ones are reported together in one message.
pub const HARNESS_REPORTS_MAX: usize = 8;

// ---- Speech-to-stroke timing (LLD §2.4) ----

/// Speech rate used to estimate when a not-yet-synthesized clause will play (about 15
/// characters a second), for op budgets only; firing always waits for real playback.
pub const ESTIMATED_SPEECH_MS_PER_BYTE: u32 = 65;

/// The orb stays in its drawing state this long after the last stroke's budget ends, so the nib
/// doesn't flicker between ops.
pub const ORB_DRAWING_LINGER_MS: u64 = 800;

// ---- Memory and views (LLD §2.8, §3.7) ----

/// Items one session may capture (memories, notes, tasks, proposals, kept proposals), the same
/// bound as the store's `SESSION_CAPTURES_MAX`. Past it `remember` and `add_task` answer "Not
/// stored" and nothing is sent, so the core and the store never disagree on what "that" is.
pub const CAPTURES_PER_SESSION_MAX: usize = 500;

/// Longest store failure detail shown under a memory problem; a longer one is cut and ends
/// with "…".
pub const MEMORY_FAILURE_DETAIL_MAX_BYTES: usize = 200;

/// Items per list the talker reads about a view on screen (sessions, tasks, notes, memories);
/// the rest are counted ("N more on screen"). Past boards list up to three times as many labels.
pub const VIEW_TALKER_ITEMS_MAX: usize = 8;

// ---- You draw (artboard 13, LLD §2.5 "Your shapes") ----

/// Points in one pen stroke. The UI samples at most this many; a longer stroke is refused
/// (dropped and counted, and the UI's preview goes away with no echo).
pub const USER_STROKE_POINTS_MAX: usize = 512;

/// Pen strokes joined into one shape (a cylinder's body and lid). Past it, the next stroke
/// starts a shape of its own.
pub const USER_SHAPE_STROKES_MAX: usize = 32;

/// Your shapes on one layer, so a drawing never leaves BoardAI no room of the layer's
/// [`ELEMENTS_PER_LAYER_MAX`]. A shape past it is refused (dropped and counted).
pub const USER_SHAPES_PER_LAYER_MAX: usize = 200;

/// Largest coordinate a drawn shape may use, in world units either way from the origin. A
/// shape past it is refused, which keeps every geometry sum far from overflow.
pub const USER_COORDINATE_MAX: i32 = 1_000_000;

/// A box or an arrow smaller than this in both directions, or a pen stroke shorter than this,
/// is a click, not a shape: refused.
pub const USER_SHAPE_MIN_UNITS: u32 = 6;

/// Words after a label lead-in ("this one is the cache") that still make a label; longer is
/// talk about the drawing, kept for the talker.
pub const USER_LABEL_WORDS_MAX: usize = 4;

/// Words that label the latest shape on their own, with no lead-in ("cache", "the API").
pub const USER_BARE_LABEL_WORDS_MAX: usize = 3;

/// What you say while drawing that is not a label, kept for the talker's turn; words past it
/// are dropped and the kept text ends with "…".
pub const DRAWING_WORDS_MAX_BYTES: usize = 1_024;

/// The description of your drawing in the talker's turn; a longer one ends with "…".
pub const DRAWING_DESCRIPTION_MAX_BYTES: usize = 2_048;
