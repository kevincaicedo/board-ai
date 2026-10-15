//! The runtime's named limits (LLD §5). Each one says what happens when it is crossed.

// ---------------------------------------------------------------------------------------------
// LLM providers (owner: runtime::llm)

/// Retries of one provider request. They happen only before the first token, so a partly
/// delivered stream is never replayed; past this count the last error is returned.
pub const PROVIDER_RETRIES_MAX: u32 = 2;

/// First retry delay; each retry doubles it and adds up to half of it again as jitter.
pub const PROVIDER_RETRY_BACKOFF_BASE_MS: u64 = 250;

/// Longest `retry-after` the runtime waits out. A longer one is not retried: the error is
/// returned with its `retry_after`, so the user hears it instead of silence.
pub const PROVIDER_RETRY_AFTER_MAX_MS: u64 = 10_000;

/// The talker must produce its first token within this time of the request, retries
/// included; past it the stream fails with `Timeout` and the user hears a failure line.
pub const TALKER_FIRST_TOKEN_DEADLINE_MS: u64 = 10_000;

/// The scribe (digests, memory) is not latency-bound, but it is still bounded: past this
/// without a first token the stream fails with `Timeout`.
pub const SCRIBE_FIRST_TOKEN_DEADLINE_MS: u64 = 60_000;

/// Silence allowed between chunks once a stream has started. Anthropic pings while it thinks,
/// so a longer gap means a dead connection: the stream fails with `Timeout`.
pub const PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 30_000;

/// TCP and TLS connect time; past it the attempt fails as `Network` (and may be retried).
pub const PROVIDER_CONNECT_TIMEOUT_MS: u64 = 5_000;

/// Time to read an error response's body; past it the error keeps its status and no detail.
pub const PROVIDER_ERROR_BODY_TIMEOUT_MS: u64 = 2_000;

/// Bytes of an error response's body that are read; the rest is ignored.
pub const PROVIDER_ERROR_BODY_MAX_BYTES: usize = 16 * 1024;

/// Characters of provider error text kept in an error's detail; the rest is cut.
pub const PROVIDER_ERROR_DETAIL_MAX_CHARS: usize = 300;

/// Total bytes of one streamed response; past it the stream fails with `BadOutput`. It also
/// bounds everything accumulated from the stream (text, tool input, raw content).
pub const PROVIDER_RESPONSE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Longest SSE line; a longer line fails the stream with `BadOutput`.
pub const SSE_LINE_MAX_BYTES: usize = 1024 * 1024;

/// Largest SSE event (its joined data lines); a larger event fails the stream with
/// `BadOutput`.
pub const SSE_EVENT_MAX_BYTES: usize = 1024 * 1024;

/// Default output budget of a talker response, thinking included. Crossing it ends the
/// response with `StopReason::MaxTokens` and the core finishes the current sentence.
pub const TALKER_MAX_TOKENS_DEFAULT: u32 = 16_000;

/// Default output budget of a scribe response; crossing it ends with `MaxTokens`.
pub const SCRIBE_MAX_TOKENS_DEFAULT: u32 = 16_000;

/// Largest `max_tokens` settings accept (the current models' output cap); a larger value is
/// a settings error.
pub const LLM_MAX_TOKENS_MAX: u32 = 128_000;

// ---------------------------------------------------------------------------------------------
// Repo tools (owner: runtime::tools)

/// Lines in one tool result; the rest are cut and counted in `more_lines`, `more_hits` or
/// `more_entries`.
pub const TOOL_RESULT_MAX_LINES: usize = 400;

/// Bytes of text in one tool result; once the next line would cross it, the rest are cut and
/// counted like lines past `TOOL_RESULT_MAX_LINES`.
pub const TOOL_RESULT_MAX_BYTES: usize = 64 * 1024;

/// Longest line in a tool result; a longer line is cut and ends with `…`.
pub const TOOL_LINE_MAX_BYTES: usize = 2_000;

/// Longest path argument; a longer one is rejected as invalid input.
pub const TOOL_PATH_MAX_BYTES: usize = 4_096;

/// Bytes of a file that `read_file` scans. Past it reading stops, so `more_lines` is a lower
/// bound for larger files.
pub const READ_FILE_SCAN_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes sniffed for a NUL to call a file binary; binary files are not read or searched.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Hits in one `search_code` result; further hits are counted in `more_hits`.
pub const SEARCH_HITS_MAX: usize = 100;

/// Files larger than this are skipped by `search_code` (generated or vendored code).
pub const SEARCH_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// Files `search_code` opens; past it the walk stops and `more_hits` is a lower bound.
pub const SEARCH_FILES_MAX: usize = 20_000;

/// Bytes `search_code` reads in total; past it the walk stops and `more_hits` is a lower
/// bound.
pub const SEARCH_SCAN_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Compiled size of a search regex; a larger pattern is rejected as invalid input.
pub const SEARCH_REGEX_SIZE_MAX_BYTES: usize = 1024 * 1024;

/// Entries in one `list_dir` result; the rest are counted in `more_entries`.
pub const LIST_ENTRIES_MAX: usize = 400;

// ---------------------------------------------------------------------------------------------
// Coding harnesses (owner: runtime::harness)

/// Read-only tasks (research, review) running at once; more wait in the queue.
pub const HARNESS_READ_TASKS_MAX: usize = 2;

/// Change tasks running at once; more wait in the queue.
pub const HARNESS_CHANGE_TASKS_MAX: usize = 1;

/// Tasks waiting for a slot, of any kind. Beyond it a task is refused at once with
/// `HarnessFailure::QueueFull`, which the core says out loud.
pub const HARNESS_QUEUE_MAX: usize = 4;

/// Research and review time; past it the task is cancelled and ends with `TimedOut`.
pub const HARNESS_RESEARCH_TIMEOUT_MS: u64 = 10 * 60 * 1_000;

/// Change time; past it the task is cancelled and ends with `TimedOut`.
pub const HARNESS_CHANGE_TIMEOUT_MS: u64 = 30 * 60 * 1_000;

/// Captured harness output (stdout and stderr, interleaved): only the last bytes are kept.
pub const HARNESS_OUTPUT_TAIL_BYTES: usize = 256 * 1024;

/// Bytes of captured stderr quoted in a failure's detail; earlier bytes are left out.
pub const HARNESS_FAILURE_DETAIL_MAX_BYTES: usize = 2 * 1024;

/// Longest JSON line read from a harness; a longer line (a huge tool result) is skipped and
/// counted, and the task continues.
pub const HARNESS_LINE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// After SIGINT, how long the harness gets to exit before it is killed.
pub const HARNESS_CANCEL_GRACE_MS: u64 = 3_000;

/// After the harness exits, how long its pipes are still read for its last words; output after
/// that (a grandchild holding a pipe) is not captured.
pub const HARNESS_EXIT_DRAIN_MS: u64 = 500;

/// Characters of one progress note; a longer note is cut at a word and ends with `…`.
pub const HARNESS_PROGRESS_MAX_CHARS: usize = 280;

/// Bytes of a harness's final summary; a longer summary is cut and ends with `…`.
pub const HARNESS_SUMMARY_MAX_BYTES: usize = 64 * 1024;

/// `path:line` citations kept from one summary; further ones are dropped.
pub const HARNESS_CITATIONS_MAX: usize = 32;

/// Distinct files reported as touched by one task; further ones are not reported.
pub const HARNESS_FILES_TOUCHED_MAX: usize = 200;

/// Time for one `git` command before a change task; past it the task fails.
pub const GIT_COMMAND_TIMEOUT_MS: u64 = 10_000;

// ---------------------------------------------------------------------------------------------
// Settings and secrets (owner: runtime::settings)

/// Largest `settings.toml`; a larger file is a settings error, never partly read.
pub const SETTINGS_FILE_MAX_BYTES: u64 = 256 * 1024;

/// Longest secret name; a longer name is a settings error.
pub const SECRET_NAME_MAX_BYTES: usize = 64;

// ---------------------------------------------------------------------------------------------
// Store: event log, vault, index, recall and the scribe (owner: runtime::store)

/// Messages waiting for the store actor. Past it, log entries are dropped and counted (a
/// `dropped` line records the count at the next flush) so logging never blocks the
/// orchestrator; requests wait for room instead, since their callers await a reply anyway.
pub const STORE_INBOX_CAPACITY: usize = 4_096;

/// Buffered log entries are written at least this often, and at session end, so a crash loses
/// at most this much of the log.
pub const EVENT_LOG_FLUSH_MS: u64 = 250;

/// Log entries buffered between flushes; reaching it flushes early.
pub const EVENT_LOG_PENDING_MAX: usize = 8_192;

/// Longest log line. A longer record is replaced by a counted `dropped` marker line, never cut
/// mid-JSON. It equals the UI's `REPLAY_LINE_BYTES_MAX`, so every line we write replays. The
/// reader skips and counts longer lines.
pub const EVENT_LOG_LINE_MAX_BYTES: usize = 1024 * 1024;

/// Board events `open_session` returns to bring a past board back; later ones are counted in
/// `board_events_skipped` and not returned.
pub const SESSION_BOARD_EVENTS_MAX: usize = 4_096;

/// Longest session id. A longer id (from the talker's `open_session`, or a hand-made folder)
/// is not a session id: the lookup finds nothing, and the scan skips the folder.
pub const SESSION_ID_MAX_BYTES: usize = 96;

/// Longest file or folder name in the vault (the file system's own limit). A longer name is not
/// a vault path, so nothing is written under it.
pub const VAULT_NAME_MAX_BYTES: usize = 255;

/// Longest slug in a vault file or folder name; a longer one is cut at a word boundary.
pub const VAULT_SLUG_MAX_BYTES: usize = 60;

/// Numbered variants (`-2`, `-3`…) tried when a name is taken; past it the write fails with
/// `StoreError::Io`.
pub const VAULT_NAME_COLLISIONS_MAX: u32 = 999;

/// Largest Markdown file the store reads (notes, projects, memory, digests). A larger file is
/// not indexed (counted in the scan report), and the store refuses to edit it.
pub const VAULT_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Lines of a Markdown file's front matter block. Past it the block is not front matter: the
/// file is read as body only, so a hand-edited file never stalls a scan.
pub const VAULT_FRONT_MATTER_LINES_MAX: usize = 64;

/// Files one vault scan looks at; past it the scan stops and reports `truncated`.
pub const VAULT_SCAN_FILES_MAX: usize = 100_000;

/// Longest one-line item (a memory, a task title, a proposal); a longer one is cut at a
/// character boundary and ends with `…`.
pub const VAULT_ITEM_MAX_BYTES: usize = 2 * 1024;

/// Longest note body; a longer note is cut and ends with `…`.
pub const VAULT_NOTE_MAX_BYTES: usize = 64 * 1024;

/// Links kept on one note or task; further links are dropped.
pub const VAULT_LINKS_MAX: usize = 16;

/// Items captured in one session (memories, notes, tasks, proposals). Past it `remember` is
/// refused with `StoreError::Limit`, which the talker hears as a failed tool call.
pub const SESSION_CAPTURES_MAX: usize = 500;

/// Text of one file the index makes searchable (a long transcript); the rest is not searchable.
pub const INDEX_TEXT_MAX_BYTES: usize = 1024 * 1024;

/// How long a SQLite statement waits on a locked index before failing with `StoreError::Index`.
pub const INDEX_BUSY_TIMEOUT_MS: u64 = 2_000;

/// Hits in one `recall` answer; lower-ranked hits are counted, not shown.
pub const RECALL_HITS_MAX: usize = 8;

/// Bytes of one `recall` answer; hits that would cross it are counted, not shown.
pub const RECALL_TEXT_MAX_BYTES: usize = 4 * 1024;

/// Search terms taken from one query; further words are ignored.
pub const RECALL_QUERY_TERMS_MAX: usize = 16;

/// Sessions in one library listing, newest first; older ones are counted in `more_sessions`.
pub const LIBRARY_ENTRIES_MAX: usize = 500;

/// Open tasks in the Now list ("a short list"); the rest are counted in `more_tasks`.
pub const NOW_LIST_MAX: usize = 12;

/// Characters of the "where we left off" recap; a longer recap is cut at a word and ends
/// with `…`.
pub const RECAP_MAX_CHARS: usize = 600;

/// Transcript bytes sent to the scribe. A longer transcript keeps its first and last parts,
/// with a marker counting the lines left out.
pub const SCRIBE_TRANSCRIPT_MAX_BYTES: usize = 48 * 1024;

/// Bytes of a scribe answer; past it the answer is junk and the minimal digest is written.
pub const SCRIBE_RESPONSE_MAX_BYTES: usize = 64 * 1024;

/// Total time for the scribe's digest; past it the minimal digest is written instead.
pub const SCRIBE_DIGEST_DEADLINE_MS: u64 = 120_000;

/// Items in one digest list (diagrams, questions, proposals, entities, relations); further
/// items from the scribe are dropped.
pub const DIGEST_LIST_ITEMS_MAX: usize = 24;

/// Characters of one digest list item; a longer item is cut and ends with `…`.
pub const DIGEST_ITEM_MAX_CHARS: usize = 300;

/// Characters of a digest title; a longer title is cut and ends with `…`.
pub const DIGEST_TITLE_MAX_CHARS: usize = 80;

/// Characters of a digest summary; a longer summary is cut and ends with `…`.
pub const DIGEST_SUMMARY_MAX_CHARS: usize = 2_000;
