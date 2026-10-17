//! Sources and research briefs (D-184, D-185, LLD §5), re-exported from `crate::limits`.

/// Sources shared for this session. Sharing another one is refused out loud ("That's as many
/// as one session can hold; remove one first") and the runtime is told to drop it.
pub const SESSION_SOURCES_MAX: usize = 16;

/// Sources kept in one project. Keeping another one is refused out loud; the vault's own list
/// is read up to this many, and the rest are counted and left out.
pub const PROJECT_SOURCES_MAX: usize = 32;

/// A source's title (its folder or file name) as the tray shows it; cut, ending with "…".
pub const SOURCE_TITLE_MAX_BYTES: usize = 80;

/// A source's path as the tray shows it (`~/…/infinitydb/crates/inf-alloc`); a longer one keeps
/// its end after `~/…/`.
pub const SOURCE_SHOWN_PATH_MAX_BYTES: usize = 120;

/// A project's name; a longer one is cut at a character.
pub const PROJECT_NAME_MAX_BYTES: usize = 60;

/// A state's reason in the tray ("3 figures unread", "Claude Code failed: …"); cut with "…".
pub const SOURCE_REASON_MAX_BYTES: usize = 160;

/// File names the harness was last seen reading, kept for the ghost names at the edge (PRODUCT
/// §8.6); the oldest leaves first.
pub const READING_GHOSTS_MAX: usize = 4;

/// A name said by voice to resolve ("the inf-alloc crate"); longer words are not a name.
pub const SPOKEN_SOURCE_NAME_MAX_BYTES: usize = 64;

/// Candidates read out when a spoken name matches several folders; the runtime sends at most
/// this many, and BoardAI asks which one.
pub const RESOLVE_CANDIDATES_MAX: usize = 4;

/// Briefs one source keeps answers for this session, by topic; past it the least recently used
/// topic is forgotten (the cache in the runtime keeps its own copy).
pub const BRIEF_TOPICS_PER_SOURCE_MAX: usize = 8;

/// A research brief's Markdown as the core parses it. The harness's summary is already bounded
/// at the same size; past it the text is cut at a line and the cut is counted.
pub const BRIEF_MAX_BYTES: usize = 64 * 1024;

/// One line of a brief; a longer line is dropped and counted.
pub const BRIEF_LINE_MAX_BYTES: usize = 2 * 1024;

/// The brief's topic (`# Brief: …`); cut with "…".
pub const BRIEF_TOPIC_MAX_BYTES: usize = 120;

/// The overview section; past it the rest of the section is dropped and counted.
pub const BRIEF_OVERVIEW_MAX_BYTES: usize = 2 * 1024;

/// Components in one brief; later ones are dropped and counted.
pub const BRIEF_COMPONENTS_MAX: usize = 48;

/// Relations in one brief; later ones are dropped and counted.
pub const BRIEF_RELATIONS_MAX: usize = 64;

/// Flows in one brief, and steps in one flow; later ones are dropped and counted.
pub const BRIEF_FLOWS_MAX: usize = 8;
pub const BRIEF_FLOW_STEPS_MAX: usize = 12;

/// A diagram block's raw text (the narration tags, kept for the talker and for staged layout,
/// T-032); past it the block is cut at a line and counted.
pub const BRIEF_DIAGRAM_MAX_BYTES: usize = 12 * 1024;

/// Component details in one brief, and the notes of one; later ones are dropped and counted.
pub const BRIEF_DETAILS_MAX: usize = 16;
pub const BRIEF_DETAIL_NOTES_MAX_BYTES: usize = 2 * 1024;

/// Citations in one brief (the section and the flows' steps together); later ones are
/// dropped and counted. Every one is read natively before its chip may show (law 6).
pub const BRIEF_CITATIONS_MAX: usize = 32;

/// Lines one citation may span; a wider range is cut to its first lines.
pub const BRIEF_CITATION_LINES_MAX: u32 = 40;

/// A component's name, kind, path or role; cut with "…".
pub const BRIEF_FIELD_MAX_BYTES: usize = 160;

/// A cited path; a longer one is not a citation.
pub const BRIEF_CITATION_PATH_MAX_BYTES: usize = 512;

/// Spoken names waiting for the runtime's answer. The runtime answers each at once; past this,
/// the oldest is forgotten and its late answer is discarded.
pub const SOURCE_RESOLVES_WAITING_MAX: usize = 4;

/// A document's digest as a reader reports it; past it the digest is cut at a character and
/// counted.
pub const DOCUMENT_DIGEST_MAX_BYTES: usize = 64 * 1024;

/// Research briefs' diagrams read and waiting to be staged (D-242): until the brief's request
/// goes out, or for a prefetch until the talker uses the brief. Past it the oldest is forgotten
/// and counted; its brief still reaches the talker, which draws with its own tags.
pub const BRIEF_DIAGRAMS_WAITING_MAX: usize = 4;
