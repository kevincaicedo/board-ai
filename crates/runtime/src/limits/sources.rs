//! Sources' limits (owner: runtime::sources, D-184, D-185), re-exported from `crate::limits`.

/// A path dropped or chosen; a longer one is refused.
pub const SOURCE_PATH_MAX_BYTES: usize = 1_024;

/// Paths one drop may carry; the rest are not looked at, and the overlay counts them.
pub const DROP_PATHS_MAX: usize = 16;

/// A single code file shared as a source. A larger one is generated or data, not code to
/// study: refused.
pub const SOURCE_CODE_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// A Markdown or text document; a larger one is refused.
pub const SOURCE_TEXT_FILE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// A PDF; a larger one is refused.
pub const SOURCE_PDF_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Files counted in a shared code folder for "23 of ~40 files"; past it the count is left
/// out.
pub const SOURCE_FILES_COUNTED_MAX: u32 = 20_000;

/// The resolver's walk for a spoken name: folders looked in, and how deep below each code
/// folder. Past either it answers with what it found.
pub const RESOLVE_FOLDERS_MAX: u32 = 5_000;
pub const RESOLVE_DEPTH_MAX: usize = 6;

/// A resolve, a project load or a citation check, from start to answer; past it the answer is
/// what was found so far (nothing, for a load: the project is reported unreadable).
pub const SOURCE_JOB_TIMEOUT_MS: u64 = 5_000;

/// A page read's wait for its reader, inside its source job's bound with half a second to spare,
/// so the job itself answers (`PagesUnread::TimedOut`, or the reader's reason) and its fallback
/// never has to; past it the talker hears the pages weren't read in time.
pub const PAGE_READ_TIMEOUT_MS: u64 = 4_500;

const _: () = assert!(
    PAGE_READ_TIMEOUT_MS < SOURCE_JOB_TIMEOUT_MS,
    "a page read answers inside its source job's bound"
);

/// Source jobs (resolves, loads, citation checks, freshness stamps) running at once; past it
/// the job's answer is given at once as empty, and logged.
pub const SOURCE_JOBS_MAX: usize = 8;

/// Briefs the session keeps in memory (session sources); past it the least recently used goes.
/// Kept sources' briefs are in the vault, so nothing is lost there.
pub const SESSION_BRIEFS_CACHED_MAX: usize = 64;

/// Files whose modification times stamp a folder's freshness, with the git HEAD; past it the
/// newest time seen so far is the stamp.
pub const STAMP_FILES_MAX: u32 = 5_000;

/// A project's `sources.md` and one brief file, read by the store; past it the file is not
/// read and the project's sources are reported unreadable (a brief: not cached).
pub const PROJECT_FILE_READ_MAX_BYTES: usize = 256 * 1024;

/// Files looked at in a project's `briefs/` folder when one source's briefs are read back; past
/// it the rest aren't looked at (a brief among them is researched again when asked). Of the
/// source's own, at most `SESSION_BRIEFS_CACHED_MAX` are read.
pub const PROJECT_BRIEF_FILES_SCANNED_MAX: usize = 4_096;

/// A source's path shown whole in the tray up to this length; longer, it keeps its last three
/// parts after `~/…/` (the tray cuts what is still too wide with an ellipsis).
pub const SOURCE_SHOWN_FULL_MAX_BYTES: usize = 44;

/// Bytes of a git ref file read for the stamp (`.git/HEAD`, a branch's ref, `packed-refs`);
/// past it the stamp has no HEAD.
pub const STAMP_GIT_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// Folders walked up from a source to find its `.git`; past it the stamp has no HEAD and a
/// brief's harness reaches only the source folder (D-226).
pub const STAMP_GIT_ANCESTORS_MAX: usize = 32;

/// Document readers running at once (the `SourceReader` seam); a document shared past it waits
/// in line, and starts when a reader ends.
pub const SOURCE_READERS_RUNNING_MAX: usize = 2;

/// Documents waiting for a reader (as many as the core holds: 16 shared and 32 kept); past it
/// the longest waiting leaves the line, logged, and is read when shared again.
pub const DOCUMENTS_WAITING_MAX: usize = 48;
