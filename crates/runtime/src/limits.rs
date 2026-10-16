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

/// Library cards that get cover art (the newest); older cards show the plain porcelain cover.
/// Each cover reads one `board.excalidraw`, so this bounds the files one library read opens.
pub const LIBRARY_COVERS_MAX: usize = 24;

/// Node boxes in one cover; later nodes are left out of the picture.
pub const COVER_SHAPES_MAX: usize = 24;

/// Kept memories per list in the memory view (about you, about your systems), newest first;
/// older ones stay in the vault and out of the view.
pub const MEMORY_VIEW_ITEMS_MAX: usize = 12;

/// Notes in the memory view and under the Now list, newest first.
pub const RECENT_NOTES_MAX: usize = 4;

/// Bytes of a session log read for a replay (the app's `session_log` command); a larger log is
/// refused with `StoreError::Limit`, and the UI says it can't replay it.
pub const SESSION_LOG_READ_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// How long quitting waits for the store to close a session still recording (minimal digest)
/// and stop; past it the app exits anyway, and the next start gives that session its digest.
pub const STORE_SHUTDOWN_TIMEOUT_MS: u64 = 3_000;

/// Sessions the store gives a minimal digest when it opens (a crash or a kill left them
/// without one), oldest first; the rest get theirs at the next open.
pub const RECOVERED_SESSIONS_MAX: usize = 16;

/// Items in one digest list (diagrams, questions, proposals, entities, relations); further
/// items from the scribe are dropped.
pub const DIGEST_LIST_ITEMS_MAX: usize = 24;

/// Characters of one digest list item; a longer item is cut and ends with `…`.
pub const DIGEST_ITEM_MAX_CHARS: usize = 300;

/// Characters of a digest title; a longer title is cut and ends with `…`.
pub const DIGEST_TITLE_MAX_CHARS: usize = 80;

/// Characters of a digest summary; a longer summary is cut and ends with `…`.
pub const DIGEST_SUMMARY_MAX_CHARS: usize = 2_000;

// ---------------------------------------------------------------------------------------------
// Session orchestrator (owner: runtime::session)

/// Inputs waiting in the orchestrator's inbox. Actors on tasks (talker, speech, tools, harness)
/// wait for room, so a stalled orchestrator slows them down instead of losing their inputs. UI
/// commands can't wait (they arrive in a command handler): when the inbox is full they are
/// dropped and counted, since a stale pointer or mic toggle is worse than a missing one.
pub const SESSION_INBOX_CAPACITY: usize = 1024;

/// Deadlines held for the core's `WakeAt`. The core asks once per distinct deadline and needs
/// only a few at a time, so crossing this is a broken invariant: the new deadline is dropped
/// and logged as an error (debug builds assert).
pub const SESSION_DEADLINES_MAX: usize = 1024;

/// Repo-tool and store queries running at once. Past it a call is answered at once with
/// `ToolFailure::Unavailable`, which the talker reads (the core allows 16 calls per reply).
pub const SESSION_TOOL_TASKS_MAX: usize = 32;

/// Harness tasks the session follows at once. The harness pool already caps running and
/// queued tasks (2 + 1, queue 4); past this a task is answered at once with
/// `HarnessFailure::QueueFull`, which the core says out loud.
pub const SESSION_HARNESS_TASKS_MAX: usize = 16;

/// How long closing a session waits for the store to flush; past it the session ends anyway
/// and the store loses at most what it had not written (it flushes every 250 ms on its own).
pub const SESSION_FLUSH_TIMEOUT_MS: u64 = 2_000;

/// Session time with no input (no voice, no command, no talker or tool activity) after which
/// the session ends and its digest is written. The next utterance starts a new session, with
/// this one's recap as context. A session muted for this long ends too, and only an unmute
/// starts the next one.
pub const SESSION_IDLE_TIMEOUT_MS: u64 = 30 * 60 * 1_000;

/// The scribe's deadline when the app quits. Past it the minimal digest is written instead, so
/// quitting never waits on a model (an idle timeout gives the scribe its full
/// `SCRIBE_DIGEST_DEADLINE_MS`: nobody is waiting then).
pub const SESSION_QUIT_DIGEST_MS: u64 = 8_000;

/// Beyond the scribe's deadline, the time ending a session may take to close its log and write
/// its digest and board (local files). Past it closing goes on; the store's shutdown still
/// writes the minimal digest.
pub const SESSION_FINISH_WRITE_MS: u64 = 2_000;

/// When the first words after an idle timeout arrive while the last session's digest is still
/// being written, its scribe is cancelled (the minimal digest is written) and the new session
/// waits at most this long for it.
pub const SESSION_FINISH_CANCEL_WAIT_MS: u64 = 1_000;

/// Bytes of the context a session starts with (where you left off, what BoardAI keeps about
/// you); past it the rest is left out at a line boundary.
pub const SESSION_CONTEXT_MAX_BYTES: usize = 4 * 1024;

/// Memory writes waiting to reach the store, in order. They are applied one at a time so
/// "forget that" always undoes the capture the core meant. Past it a write is dropped and
/// logged as an error (the store would have to be stalled for hundreds of captures).
pub const STORE_WRITES_CAPACITY: usize = 1_024;

/// Ops of a past board brought back by `open_session`; the rest are counted in
/// `ops_left_out` and not shown, so the view stays well under the event log's line limit.
pub const PAST_BOARD_OPS_MAX: usize = 1_024;

/// Voice time per word of a typed utterance, as if spoken: the voiced length the core sees, so
/// the barge-in rules that count voice time apply as they would to speech.
pub const TYPED_MS_PER_WORD: u32 = 280;

/// Board labels kept as recent terms for speech-to-text, newest first; past it the oldest is
/// forgotten. Whisper's prompt keeps fewer still (`STT_VOCABULARY_TOKENS_MAX`).
pub const VOCABULARY_RECENT_TERMS_MAX: usize = 32;

/// Longest label taken as a term; a longer one (a sentence used as a label) is left out.
pub const VOCABULARY_TERM_MAX_BYTES: usize = 64;

// ---------------------------------------------------------------------------------------------
// UI bridge (owner: runtime::bridge)

/// One UI frame: events of the steps within it go out together, stamped with the latest step's
/// time. The first step after a quiet frame goes out at once.
pub const BRIDGE_FRAME_MS: u64 = 16;

/// Events in one batch. Captions, the orb, the thread and the camera already coalesce (latest
/// wins). A frame that still holds more sends a full batch at once and continues in the next
/// one with the same stamp: drawing, layer and surface events are never dropped. Counted.
pub const BRIDGE_BATCH_EVENTS_MAX: usize = 256;

/// Steps waiting for the bridge. The orchestrator waits for room: losing a drawing op would
/// break the board, and the bridge only hands batches to a non-blocking sink.
pub const BRIDGE_QUEUE_STEPS: usize = 256;

// ---------------------------------------------------------------------------------------------
// Virtual voice (owner: runtime::session::virtual_voice)

/// Commands waiting for the virtual voice. Past it `synthesize` is refused, and the
/// orchestrator answers that clause with `SynthesisFailed`.
pub const VIRTUAL_VOICE_COMMANDS_MAX: usize = 1024;

/// How often playback positions are published, as speech out coalesces them (LLD §2.1).
pub const PLAYBACK_REPORT_MS: u64 = 20;

/// The fade after `StopVoice`; audio keeps playing through it, so it counts as heard.
pub const VOICE_STOP_FADE_MS: u64 = 30;

/// The virtual voice's speaking rate at speed 1 (about 15 characters a second, the core's
/// estimate for an unsynthesized clause).
pub const VIRTUAL_VOICE_MS_PER_BYTE: u32 = 65;

/// Pauses the virtual voice adds after a comma and after a sentence end.
pub const VIRTUAL_VOICE_COMMA_PAUSE_MS: u32 = 150;
pub const VIRTUAL_VOICE_SENTENCE_PAUSE_MS: u32 = 300;

/// Fastest and slowest speed factors the virtual voice accepts; others are clamped.
pub const VIRTUAL_VOICE_SPEED_MIN: f32 = 0.25;
pub const VIRTUAL_VOICE_SPEED_MAX: f32 = 8.0;

// ---------------------------------------------------------------------------------------------
// In-memory store and dev scripts (owner: runtime::session)

/// Memories the in-memory store keeps for `recall`; past it the oldest is forgotten.
pub const MEMORY_STORE_MEMORIES_MAX: usize = 1_000;

/// Matches one `recall` returns from the in-memory store; the rest are counted.
pub const MEMORY_STORE_RECALL_HITS_MAX: usize = 10;

/// Lines in a dev script; a longer script is rejected.
pub const SCRIPT_LINES_MAX: usize = 1_000;

/// Bytes of one dev script line (the core keeps 4 KB of an utterance); a longer line rejects
/// the script.
pub const SCRIPT_LINE_MAX_BYTES: usize = 4 * 1024;

/// A dev script waits at most this long for BoardAI to finish or start speaking; past it the
/// next line is said anyway, with a warning.
pub const SCRIPT_WAIT_MAX_MS: u64 = 120_000;

// ---------------------------------------------------------------------------------------------
// Logs (owner: runtime::logging)

/// Bytes of one field value in a log line; the rest is cut and ends with `…`.
pub const LOG_FIELD_MAX_BYTES: usize = 512;

/// Bytes of one log line; the rest is cut and ends with `…`.
pub const LOG_LINE_MAX_BYTES: usize = 4 * 1024;

// ---------------------------------------------------------------------------------------------
// Audio I/O (owner: runtime::audio)

/// The mic ring, at 16 kHz. When speech in falls this far behind, the oldest frames are
/// overwritten and counted as dropped; the real-time thread never waits.
pub const MIC_RING_MS: u32 = 2_000;

/// The voice ring, at 24 kHz. When it is full, synthesized audio waits in the speech-out actor
/// until playback frees room.
pub const VOICE_RING_MS: u32 = 4_000;

/// A gain change (a duck, or its release) reaches its target over this time, ramped per sample
/// so it never clicks.
pub const VOICE_GAIN_RAMP_MS: u32 = 10;

/// `StopVoice` fades the voice out over this time, then drops whatever is left in the ring.
pub const VOICE_FADE_MS: u32 = 30;

/// Phases of the sample-rate converter (the reduced up-sampling factor). A device rate that
/// needs more is refused with `AudioError::UnsupportedRate`; every common rate needs at most 441.
pub const RESAMPLER_PHASES_MAX: u32 = 1_024;

/// Frames one CoreAudio callback may ask for; a larger request is answered with silence and
/// counted, never with an allocation on the real-time thread.
pub const AUDIO_CALLBACK_FRAMES_MAX: usize = 8_192;

/// Audio devices read when looking for the built-in microphone; devices past it are not
/// considered (a Mac has a handful).
pub const AUDIO_DEVICES_MAX: usize = 256;

/// Largest audio file read as mic input; a larger one is refused before it is read.
pub const AUDIO_FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Longest audio file played as mic input (a clip is an utterance); a longer one is refused.
pub const AUDIO_FILE_MAX_MS: u64 = 120_000;

/// Clips waiting to be spoken into the mic ring by a file mic; past it `play` is refused with
/// `FileMicError::Full`.
pub const FILE_MIC_QUEUE_CLIPS: usize = 8;

// ---------------------------------------------------------------------------------------------
// Speech out (owner: runtime::speech_out)

/// Clauses synthesized ahead of what has been heard; past it synthesis waits for playback.
pub const SPEECH_LOOKAHEAD_CLAUSES_MAX: usize = 4;

/// Audio synthesized ahead of what has been heard; past it synthesis waits for playback.
pub const SPEECH_LOOKAHEAD_MS_MAX: u32 = 15_000;

/// Playback positions are reported at most this often (latest wins, through a `watch`).
pub const PLAYBACK_REPORT_INTERVAL_MS: u64 = 20;

/// Commands waiting for the speech-out actor; a sender waits when it is full.
pub const SPEECH_OUT_COMMAND_QUEUE: usize = 64;

/// Speech-out inputs (`ClauseReady`, `PlaybackDrained`, `VoiceStopped`) waiting for the
/// orchestrator; the actor waits when it is full, so none is ever dropped.
pub const SPEECH_OUT_EVENT_QUEUE: usize = 64;

/// Time for the audio device to acknowledge a stop. Past it `VoiceStopped` is answered from the
/// sample counter, and new audio waits until the device answers.
pub const VOICE_STOP_ACK_TIMEOUT_MS: u64 = 250;

/// Longest audio of one clause; the rest is cut and counted (a 300-byte clause is ~25 s).
pub const TTS_CLAUSE_AUDIO_MAX_MS: u32 = 40_000;

/// Time to synthesize one clause; past it the engine fails with `Timeout` and the fallback
/// voice is tried.
pub const TTS_CLAUSE_TIMEOUT_MS: u64 = 15_000;

/// The macOS system voice must deliver its first audio within this time. Its callbacks run on
/// the main thread, so a process whose main thread runs no event loop fails here, out loud,
/// instead of hanging.
pub const SYSTEM_VOICE_FIRST_AUDIO_TIMEOUT_MS: u64 = 3_000;

// ---------------------------------------------------------------------------------------------
// Speech in (owner: runtime::speech_in)

/// How often the VAD thread looks for new mic audio when it has none (it sleeps in between).
pub const VAD_POLL_INTERVAL_MS: u64 = 8;

/// Audio kept before the voice onset and handed to speech-to-text with the utterance, so the
/// first word is not clipped.
pub const SPEECH_PREROLL_MS: u32 = 300;

/// A partial transcript is decoded at most this often while you speak; a partial whose turn
/// comes while a decode is busy is skipped.
pub const STT_PARTIAL_INTERVAL_MS: u64 = 500;

/// Longest utterance kept for speech-to-text (Whisper's window). Audio past it is dropped and
/// counted; the core ends the turn at the same length.
pub const STT_UTTERANCE_MAX_MS: u32 = 30_000;

/// Whisper tokens of vocabulary given as the initial prompt; terms past it are left out.
pub const STT_VOCABULARY_TOKENS_MAX: usize = 200;

/// Messages (32 ms windows and controls) waiting for the speech-to-text thread: about 32 s.
/// Past it a window is dropped and counted.
pub const STT_QUEUE_MESSAGES: usize = 1_024;

/// Speech-in inputs waiting for the orchestrator. When full, a partial transcript is dropped
/// and counted (a newer one follows); voice start and end and final transcripts wait.
pub const SPEECH_IN_EVENT_QUEUE: usize = 64;

/// Repo symbols offered to speech-to-text, best first; the rest are left out (the 200-token
/// prompt keeps fewer still).
pub const VOCABULARY_REPO_TERMS_MAX: usize = 64;

/// Source files read for repo symbols; past it the scan stops with what it has.
pub const VOCABULARY_SCAN_FILES_MAX: usize = 2_000;

/// Bytes read for repo symbols in all; past it the scan stops with what it has.
pub const VOCABULARY_SCAN_BYTES_MAX: u64 = 16 * 1024 * 1024;

/// Bytes read of one source file for repo symbols; the rest of the file is not read.
pub const VOCABULARY_SCAN_FILE_MAX_BYTES: u64 = 256 * 1024;

/// Distinct identifiers counted during the scan; later new names are not counted (a name
/// already counted still is).
pub const VOCABULARY_IDENTIFIERS_MAX: usize = 100_000;

// ---------------------------------------------------------------------------------------------
// Model files (owner: runtime::models)

/// Silence from the server during a model download; past it the download fails with `Network`
/// and the partial file is kept for resuming.
pub const MODEL_DOWNLOAD_IDLE_TIMEOUT_MS: u64 = 30_000;

/// Progress is reported at most every this many bytes (and at the end).
pub const MODEL_PROGRESS_STEP_BYTES: u64 = 1024 * 1024;

/// Bytes of one ElevenLabs response; past it the clause fails with `BadOutput` (a 300-byte
/// clause is about 2 MB of base64 audio).
pub const ELEVENLABS_RESPONSE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Longest line of an ElevenLabs response; a longer line fails the clause with `BadOutput`.
pub const ELEVENLABS_LINE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Longest ElevenLabs voice id (theirs are 20 characters); a longer or non-alphanumeric id is
/// refused with `Provider` before anything is sent.
pub const ELEVENLABS_VOICE_ID_MAX_BYTES: usize = 64;

/// Phoneme tokens in one Kokoro run (the model's 512 positions minus the two pads). A longer
/// clause is split at a word boundary and synthesized in parts.
pub const KOKORO_TOKENS_MAX: usize = 510;

/// Largest pronunciation lexicon read (the pinned file is 3 MB); a larger file makes Kokoro
/// unavailable and the system voice speaks instead.
pub const LEXICON_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// How many times an unknown word is split (camelCase, compounds) before the letter-to-sound
/// rules take what is left.
pub const G2P_SPLIT_DEPTH_MAX: u32 = 3;

/// Digit runs longer than this are read digit by digit (a phone number, an id), not as one
/// number.
pub const NUMBER_DIGITS_MAX: usize = 15;
