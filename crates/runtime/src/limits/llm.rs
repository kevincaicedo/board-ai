//! LLM providers' limits (owner: runtime::llm), re-exported from `crate::limits`.

use super::SESSION_IDLE_TIMEOUT_MS;

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

/// Longest line of a newline-delimited JSON stream (Ollama's native chat); a longer line fails
/// the stream with `BadOutput`.
pub const NDJSON_LINE_MAX_BYTES: usize = 1024 * 1024;

/// The context window the talker asks a local Ollama model for, in tokens. Ollama loads models
/// with 4,096 by default, and the system prompt and tool schemas alone take ~3,600 of them
/// (`boardai_core::limits::PROMPT_AND_TOOLS_MAX_BYTES`): with
/// a conversation on top, Ollama dropped the front of the prompt (no drawing at all) or stopped
/// answers at the window (26 of 49 `just e2e` answers). 16,384 leaves ~12,700 tokens of history,
/// about 25 voice turns; past it Ollama drops the oldest messages and keeps the system prompt.
/// Changing it reloads the model, so it is one value for every request.
pub const OLLAMA_CONTEXT_TOKENS: u32 = 16_384;

/// How long Ollama keeps the talker's model loaded after a request, in seconds: the session's
/// idle limit, so a pause within a session never pays a ~5 s reload and a ~5 s prefill of the
/// system prompt (past the talker's 10 s first-token deadline, the answer would fail).
pub const OLLAMA_KEEP_ALIVE_SECONDS: u64 = SESSION_IDLE_TIMEOUT_MS / 1_000;

/// Longest tool call in gemma4's own call syntax (`list_dir{path:<|"|>.<|"|>}`) that the Ollama
/// adapter holds back from speech while it is open (D-191): far past any talker call (a path, a
/// query, a note). Past it the call is malformed: dropped and counted once it showed native
/// syntax (`call:` or a `<|"|>` string), given back as text otherwise, and the text after it
/// is text again.
pub const OLLAMA_NATIVE_CALL_MAX_BYTES: usize = 4_096;

/// Tool calls read from gemma4's text in one response (D-191): what the core takes from one
/// response (`TOOL_CALLS_PER_RESPONSE_MAX`). Past it a call is still kept out of speech, but
/// dropped and counted instead of delivered.
pub const OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX: usize =
    boardai_core::limits::TOOL_CALLS_PER_RESPONSE_MAX;

/// Default output budget of a talker response, thinking included. Crossing it ends the
/// response with `StopReason::MaxTokens` and the core finishes the current sentence.
pub const TALKER_MAX_TOKENS_DEFAULT: u32 = 16_000;

/// Default output budget of a scribe response; crossing it ends with `MaxTokens`.
pub const SCRIBE_MAX_TOKENS_DEFAULT: u32 = 16_000;

/// Largest `max_tokens` settings accept (the current models' output cap); a larger value is
/// a settings error.
pub const LLM_MAX_TOKENS_MAX: u32 = 128_000;

/// Output budget of an OpenRouter preset talker, reasoning included: ~3,000 spoken words, past
/// any answer measured. OpenRouter holds credit for the whole budget and routes a low balance
/// away from Cerebras at 16,000 (LLD §3.3). Crossing it ends the answer with `MaxTokens`.
pub const OPENROUTER_TALKER_MAX_TOKENS: u32 = 4_096;

/// Providers in an OpenRouter route (`route` in settings, `provider.order` on the wire); a
/// longer route is a settings error.
pub const OPENROUTER_ROUTE_MAX: usize = 8;

/// Longest provider slug in a route (`cerebras`, `deepinfra/turbo`); a longer one is a
/// settings error.
pub const OPENROUTER_SLUG_MAX_BYTES: usize = 64;

/// Time for a model list (or one model's providers) to arrive whole; past it, `Network`.
pub const MODEL_LIST_TIMEOUT_MS: u64 = 8_000;

/// Largest model list read (OpenRouter's was 665 KB in 2026-10); past it, `BadOutput`.
pub const MODEL_LIST_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Models kept from one list (OpenRouter listed 372); the rest are counted in `left_out`.
pub const MODEL_LIST_MODELS_MAX: usize = 1_000;

/// Largest list of one model's providers (21–25 KB for the presets); past it, `BadOutput`.
pub const MODEL_PROVIDERS_MAX_BYTES: usize = 512 * 1024;

/// Providers kept for one model; the rest are counted in `left_out`.
pub const MODEL_PROVIDERS_MAX: usize = 64;

/// Longest model id, display name, provider name or slug kept from a list; an entry with a
/// longer id or name is left out and counted, a longer optional field is dropped.
pub const MODEL_TEXT_MAX_BYTES: usize = 200;

/// How long a fetched list is reused before the next ask fetches it again. Lists and prices
/// change rarely; a picker opened twice in a session asks OpenRouter once.
pub const MODEL_LIST_CACHE_MS: u64 = 10 * 60 * 1_000;

/// How long Ollama's model list is reused: it changes as soon as a model is pulled.
pub const OLLAMA_LIST_CACHE_MS: u64 = 5_000;

/// Models whose provider lists are cached at once; asking for one more forgets the oldest.
pub const MODEL_PROVIDERS_CACHED_MAX: usize = 32;

/// Time the talker's warm-up at session open may take (a cold Ollama: ~4 s load, ~3 s
/// prefill); past it the warm-up is dropped and the first request does the rest.
pub const TALKER_WARM_UP_TIMEOUT_MS: u64 = 30_000;

/// Bytes of a warm-up's answer that are read; the rest is dropped with the connection.
pub const TALKER_WARM_UP_MAX_BYTES: usize = 64 * 1024;
