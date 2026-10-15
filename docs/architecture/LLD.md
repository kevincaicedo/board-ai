# BoardAI · Low-level design

> Status: **draft for review** · 2026-10-15 · Diagrams: [`lld.excalidraw`](lld.excalidraw), panels A–F (open at [excalidraw.com](https://excalidraw.com) → menu → Open) · High level: [HLD.md](HLD.md)

This document gives the types, algorithms, limits and invariants behind the [HLD](HLD.md). The Rust signatures are sketches: names and shapes are intended, details settle in code. When the code and this document disagree, the code is fixed or the document is updated in the same change.

## 1. Repository and dependency rules (panel A)

```
boardai/
├─ crates/
│  ├─ core/      boardai-core     pure domain: no I/O, async, clock or randomness
│  ├─ runtime/   boardai-runtime  all I/O adapters, actors, the session driver; no Tauri
│  └─ app/       boardai-app      Tauri 2 shell: window, permissions, keychain, transport
├─ ui/           Preact + TypeScript (Vite); the canvas and orb renderers are imperative TS
├─ docs/         PRODUCT, PLAN, DECISIONS, architecture/
└─ justfile      `just check` is the one entry point for every check
```

Dependencies only point one way: `app → runtime → core`. The UI never imports Rust; it gets protocol types generated from `core` (ts-rs) and talks to the runtime through the app's transport.

How each rule is enforced, using the strongest mechanism available:

| Rule | Enforced by |
|---|---|
| Core has no I/O, async or Tauri | Cargo: `core` has no such dependencies |
| Core never reads the clock or randomness | `clippy.toml` `disallowed-methods` (`Instant::now`, `SystemTime::now`, `rand::*`) with `-D warnings` |
| No `unsafe` outside the audio FFI leaf | `#![forbid(unsafe_code)]` in `core` and `app`. In `runtime`, `#![deny(unsafe_code)]` with a module-scoped allow on `audio::coreaudio` only. |
| No wildcard `match` on our own enums | `clippy::wildcard_enum_match_arm` denied in all crates |
| No `unwrap`/`expect` on operational errors | `clippy::unwrap_used` and `clippy::expect_used` denied outside tests; an invariant-backed `expect` carries a `reason` |
| Functions ≤ 70 lines, files ≤ 800 lines | `clippy::too_many_lines` (threshold 70). The file limit is checked in `just check`. |
| Protocol types are generated, never hand-written | `just check` regenerates them and fails on any diff |
| Dependencies are few and deliberate | A new crate or npm package needs a row in [DECISIONS.md](../DECISIONS.md); `cargo deny` checks licenses and advisories |

## 2. Core

### 2.1 Shape

```rust
/// Milliseconds since the session started, on the runtime's monotonic clock.
pub struct SessionTime(u64);

pub struct Core { /* conversation, thread, board, schedule, memory candidates */ }

impl Core {
    pub fn new(config: CoreConfig) -> Self;
    /// Once, when the session opens: the initial orb and thread, and the session context.
    pub fn start(&mut self, now: SessionTime, effects: &mut Vec<Effect>);
    /// The only way state changes. Same inputs at the same times → same effects.
    pub fn step(&mut self, now: SessionTime, input: Input, effects: &mut Vec<Effect>);
}
```

The runtime must not deliver talker inputs from a stream after it applied that stream's `CancelTalk`; inputs carry no stream id, so the orchestrator tags each stream with a generation and drops the inputs of a cancelled one at its inbox (D-034).

**Inputs** (runtime → core):

| Input | From | Carries |
|---|---|---|
| `VoiceStart`, `VoiceEnd { voiced_ms }` | speech in (VAD) | |
| `Transcript { text, is_final }` | speech in (STT) | |
| `Pointer { target, at }` | UI | A board element, a code line, or nothing |
| `TalkerDelta { text }` | talker | Streamed narration text |
| `TalkerToolCall { id, call }` | talker | A validated tool call |
| `TalkerDone { stop }` | talker | `EndTurn`, `ToolUse`, `MaxTokens`, `Refusal`, `Error` |
| `ClauseReady { clause, timings }` | speech out | Word timings; exact or estimated |
| `SynthesisFailed { clause }` | speech out | Answers `Synthesize` instead of `ClauseReady`. The clause is skipped: its ops are never drawn, its text shows as a `Problem`, and the reply goes on |
| `PlaybackAt { clause, offset_ms }` | speech out | Coalesced; at most one per 20 ms |
| `PlaybackDrained` | speech out | |
| `ToolResult { id, result }`, `HarnessEvent { task, event }` | runtime | |
| `Command(UiCommand)` | UI | Mute, push-to-talk, open library… |
| `Tick` | runtime | A deadline the core asked for |

**Effects** (core → runtime): `Duck { gain }`, `StopVoice`, `Synthesize { clause }`, `Talk { request }`, `CancelTalk`, `RunTool { id, call }`, `RunHarness { task }`, `CancelHarness { task }`, `Ui(UiEvent)`, `Log(Record)`, `Remember(MemoryWrite)`, `WakeAt(SessionTime)`.

The runtime applies effects in order. The core never waits: anything slow is an effect now and an input later.

### 2.2 Conversation state machine (panel E)

Detours are not states. They are a stack inside the thread (§2.6), and speaking inside a detour is still `Speaking`.

| State | Input | Next | Effects |
|---|---|---|---|
| `Idle` | `VoiceStart` | `Listening` | orb → listening |
| `Listening` | `Transcript` (partial) | `Listening` | caption |
| `Listening` | `VoiceEnd`, and the turn sounds finished | `Thinking` | `Talk` |
| `Listening` | `VoiceEnd`, and the turn sounds unfinished | `Listening` | `WakeAt(+1500 ms)` |
| `Listening` | `Tick` (grace expired) | `Thinking` | `Talk` |
| `Thinking` | first clause synthesized | `Speaking` | orb → speaking, captions start |
| `Thinking` | `VoiceStart` | `Listening` | `CancelTalk`. The new words join the same turn. |
| `Speaking` | `PlaybackAt` | `Speaking` | fire due ops (§2.4) |
| `Speaking` | `VoiceStart` | `Ducked` | `Duck { gain: 0.25 }`, `WakeAt(+900 ms)` |
| `Ducked` | classified as a backchannel | `Speaking` | `Duck { gain: 1.0 }`, orb acknowledges |
| `Ducked` | classified as an interruption, or `Tick` | `Listening` | `StopVoice`, `CancelTalk` if streaming, board freeze, commit heard text (§2.7) |
| `Speaking` | `PlaybackDrained` and the talker is done | `Idle` | orb → idle |
| any | `Command(Mute)` | `Muted` | `StopVoice`, `CancelTalk` |
| `Muted` | `Command(Unmute)` | `Idle` | |

**End of turn.** The turn ends after ≥ 600 ms of silence, unless the transcript ends on a continuation word ("and", "so", "because", "but", "or", "then", "um", "uh", "like", "the", "a", "to", "of"). Then it waits up to 1500 ms. Utterances are capped at 30 s.

**Backchannel or interruption.** The decision is deterministic, and every lexicon is a table in `conversation::cues` that its tests iterate.

It is an **interruption** if any of these hold:

- (a) The first words contain a cue: wait, stop, hold on, hang on, what, why, how, no, actually, sorry, pause, slower, repeat, back, skip.
- (b) Four or more words.
- (c) At least 700 ms of voice.
- (d) A pointer target was set within the last 1.5 s.

It is a **backchannel** if `VoiceEnd` arrives with less than 700 ms of voice and the transcript is empty or in the backchannel lexicon: mm-hm, uh-huh, yeah, yes, right, ok, okay, sure, got it, I see, nice.

If neither applies within 900 ms of onset, it is an interruption. A person who keeps talking wants the floor.

A cue said too fast for a partial ("wait" in 300 ms) is first taken for a backchannel; its final transcript, arriving within 600 ms of that decision, still interrupts.

**Push-to-talk.** While the key is held the user keeps the floor through pauses; releasing it ends the turn as soon as the final transcript is in. Pressing it while BoardAI speaks interrupts at once, without ducking. In push-to-talk mode, voice without the key is ignored.

**Joining a turn.** Thinking + `VoiceStart` joins the new words to the same turn only when nothing of the answer was heard yet. The user's message enters the history right before the turn's first assistant content, so joined words become one message.

**Mute** stops the voice and the stream and drops the utterance in progress. If that utterance had words, it closes a pending change gate (I9): words cut off by mute can cancel a change, never confirm one.

**Orb.** Ducked shows Listening. While speaking, the orb shows Drawing from a stroke's start until 800 ms after the last stroke's budget ends, so the nib doesn't flicker between ops.

### 2.3 Narration script

The talker writes spoken text with self-closing tags inline. Text outside tags is speech. A tag sits right before the word it should start on.

```
Then it reaches the <node id="lb" label="Load balancer" kind="service" near="cdn" dir="right"/>
<edge from="cdn" to="lb"/>load balancer, which spreads traffic across
<node id="api" label="API pod" kind="service" near="lb" dir="right" stack="3"/>three API pods.
```

| Tag | Attributes | Effect |
|---|---|---|
| `plan` | `steps="Client\|Edge\|LB"` | Sets the thread's steps |
| `step` | `n` | Advances the thread |
| `node` | `id`, `label`, `kind` (service, db, queue, actor, group, note), `near`, `dir` (right, left, above, below), `in`, `stack` | Draws a node |
| `edge` | `from`, `to`, `label`, `style` (solid, dashed) | Draws a connector |
| `annotate` | `target`, `mark` (circle, underline, arrow, bracket), `text` | An orange pen mark |
| `strike` | `target`, `text` | Strikes through the label and rewrites it |
| `focus` | `target` | Moves the camera and the depth-of-field focus |
| `detour` / `resume` | `title` / — | Pushes or pops a detour layer |
| `code` | `path`, `lines="42-52"` | Opens a code card. The path must come from a read in this session. |
| `line` | `n` | Moves the code highlight |
| `trace` | `path="api>redis"` | A dot travels the edges in sync with the highlighted line |
| `compare`, `cell` | `options`, `criteria` / `row`, `col`, `mark` (yes, no, depends, text) | Opens and fills a compare card |
| `close` | `surface` | Dismisses a surface |

**Parser.** A streaming character state machine (`Text → TagOpen → Name → Attributes → Value`) with an explicit bound.

- **Split-invariant.** Feeding any split of the input gives the same result as feeding it whole. This is a property test over every golden file.
- **Tolerant.** An unknown tag, a bad attribute or a tag over 512 bytes is dropped and counted. The surrounding speech continues. Text that only resembles a tag ("a < b", "count<limit then") is given back as speech: a malformed tag is dropped only when its name is a known tag or it already has an attribute.
- **Never recursive, never blocking.** Tags carry no nesting. Text is released to text-to-speech as soon as a clause boundary is seen.

**Speakable text.** Captions show the text as written. Text-to-speech gets a normalized version: `getUserByID` → "get user by ID", `service.go` → "service dot go", `L42` → "line 42". Normalization is table-driven and keeps an offset map, so timings in speakable offsets map back to caption offsets.

**Clauses.** Text is cut at sentence ends, at a newline, or at a comma after ≥ 40 bytes, so the first clause reaches text-to-speech early. A clause is capped at 300 bytes, cut at its last space. An op is released right after the clause holding its anchor, so an anchor never names a clause that could still change.

### 2.4 Speech-to-stroke sync (panel C)

1. Each op gets an **anchor**: `(clause, char_offset)`, the position of the first spoken character after the tag.
2. Text-to-speech returns **word timings** per clause: `Vec<(char_offset, ms)>`.
   - `Exact` timings come from provider alignment (ElevenLabs, Kokoro durations, system voice markers).
   - `Estimated` timings come from character rate scaled to the real audio length.
3. The runtime reports `PlaybackAt { clause, offset_ms }` from the audio sample counter, minus the device's output latency.
4. The core maps the playback time to a spoken character offset. Every op whose anchor is at or before that offset fires as `OpStart { op, budget_ms }`.
5. `budget_ms` is the estimated time until the next op's anchor, clamped to 150–2500 ms. The UI finishes the stroke within the budget, so drawing never piles up.
6. Drawing ops start at least 120 ms apart (whether or not speech separates them), and strictly in narration order.
7. Ops anchored in a clause that never played (because it was cut) are never drawn.

**Invariant:** an op never starts before the audio of its anchor word has played. The simulator checks this on every run.

### 2.5 Board model and layout

- A board is a stack of **layers**: the main layer, plus one per open detour.
- Each **element** has a stable id, scoped to its layer, plus a kind, a label, geometry, and a state: `Pending` (laid out, waiting for its drawing slot), `Drawing`, `Drawn`, `Frozen { progress }` or `Struck`. An interruption removes `Pending` elements; the user never saw them.
- A repeated id updates the element (a relabel or a strike); it never duplicates it.
- **Layout** turns meaning into geometry on a grid of cells sized per kind.
  - `near X dir right` takes the first free cell to the right of X in the same row. A collision nudges further along the same direction.
  - Groups grow to bound their children. A grown group is sent again as a `Node` op with the same id and op, which the UI treats as a redraw of that element.
  - Edges join the nearest sides, either straight or with one curve.
- **Stability:** layout only places new ids. It never moves an existing element. When the board outgrows the view, the camera pans instead of rearranging.
- **Sizes:** label widths come from a metrics table for the handwritten font, kept in the core. The core outputs final geometry, and the UI only draws it.
- **Talker summary.** The core builds a compact text form of the board for each request, for example `main: client(actor) → cdn → lb → api×3; detour 1 "cache-aside": api → redis (hit) …`.
- **Export:** each layer becomes an `.excalidraw` frame at session end.

### 2.6 Thread and detours

- The **thread** is the plan (steps and the current step) plus a **detour stack** with a depth limit of 3. A fourth detour gets a spoken "let's finish this one first".
- Each detour owns a board layer.
- `resume` pops a detour. The UI returns the main board to focus, and then the frozen op continues (§2.7).

### 2.7 Interruption and commit-what-was-heard (panel D)

When an interruption is confirmed:

1. `StopVoice`. The runtime fades out over 30 ms and reports the last spoken character offset. That offset defines the **heard prefix**; inside a word it is interpolated from the word timings ("…checks Re"). If no answer comes within 500 ms, the last reported playback position is used; it is never ahead of the audio.
2. The board freezes. The op being drawn becomes `Frozen { progress }`, and ops not yet started are dropped.
3. The conversation is committed. History is **append-only**, as Claude's preserved-thinking check and prompt caching both require. Two cases:
   - **Generation still streaming:** cancel the stream. Append an assistant message containing only the heard prefix (text plus the tags of ops that were drawn, no thinking blocks), then the user's message.
   - **Generation already complete** (the voice lagged behind): leave the stored assistant message untouched. Start the next user message with a note: `[You were interrupted. The user heard you up to: "…checks Re". Nothing after that was heard.]`
4. The user's message carries the utterance, the pointer target (if any), the thread position, and the frozen op.
5. The talker answers. It usually opens a `detour`, but it may also redirect, correct with a `strike`, or apply a control such as "slower".
6. On `resume`:
   - The UI pops the detour layer.
   - The frozen op finishes first.
   - The talker says a bridge sentence ("So, back to the load balancer…").
   - New ops continue. Dropped ops are replanned by the talker, never replayed blindly.

The `HeardPrefix` type can only be constructed from a playback offset, so no code path can commit text the user didn't hear.

### 2.8 Memory policy

The policy is a pure function from a candidate memory to an action. The table is in [PRODUCT §7.4](../PRODUCT.md); in code it is a single `match` on `MemoryKind`:

- `Keep` covers the session record, learning signals, and what you stated about yourself or your systems.
- `Propose` covers anything inferred. It lands in the digest and is kept after a "yes".
- `OnRequest` covers notes and tasks.
- `Never` covers raw audio and secrets.

Secret redaction runs at the store boundary, so nothing reaches disk unredacted. It matches common key formats (`sk-…`, `ghp_…`, AWS keys, PEM blocks, `password=`), and a planted-secret test proves each pattern.

### 2.9 Invariants

| # | Invariant | Enforced by |
|---|---|---|
| I1 | An op never starts before its anchor word's audio has played | Simulator property |
| I2 | Committed assistant text is exactly the heard prefix | Type (`HeardPrefix`) |
| I3 | The LLM conversation is append-only | Type (`Conversation` has `push` and no edit or remove) |
| I4 | Detour depth ≤ 3, and pushes and pops balance by session end | Type (bounded stack) + simulator |
| I5 | At most one talker stream in flight | State owns `Option<TalkerTurn>`; `CancelTalk` precedes `Talk` (simulator) |
| I6 | Existing board elements never move (a group's box only grows) | Layout API places only new ids (test) + simulator |
| I7 | A backchannel never stops the voice | Table test over the lexicon |
| I8 | A secret never reaches the store | Boundary redaction + planted-secret test |
| I9 | A code change never runs without your spoken yes | Deterministic gate in core (§3.5) + test |
| I10 | A "don't remember this session" leaves nothing in the vault | Store test |

## 3. Runtime

### 3.1 Actors and channels (panel B)

| Actor | Runs on | Talks through | Notes |
|---|---|---|---|
| **Audio I/O** | CoreAudio's real-time thread | Single-producer, single-consumer rings (`rtrb`): mic in (16 kHz mono f32, 2 s), voice out (24 kHz, 4 s); atomics for samples played and the duck gain | No allocation, locks or system calls in the callback |
| **Speech in** | two dedicated threads (VAD, Whisper) | Mic ring → orchestrator inbox | Silero VAD on 32 ms windows. Whisper decodes on its own thread, one at a time; a partial is skipped while a decode is busy. |
| **Orchestrator** | tokio task (`runtime::session`) | One bounded inbox (`mpsc`, 1024) | Owns `Core`. Steps each input at the clock's session time, then applies the effects in order. Playback positions arrive on a `watch` (latest wins), never queued. Actors wait for room in the inbox; UI commands can't wait and are dropped and counted when it is full. `WakeAt` deadlines are kept in a set and the earliest becomes one `Tick`. An answer the orchestrator gives itself (a clause the voice refused becomes `SynthesisFailed`, a refused `StopVoice` becomes `VoiceStopped { last: None }`, a tool that can't run) is stepped before it waits again. Closing cancels the talker, tool and harness tasks, deadlines and the voice, then flushes the store. |
| **Talker** | tokio task | Effects in, inputs out | One stream, tagged with a generation; `CancelTalk` drops it, and the orchestrator drops any of its inputs still in the inbox |
| **Speech out** | tokio task + one inference thread | `SpeechOut` port; answers through a `SpeechOutLink`. Synthesizes up to 4 clauses (≤ 15 s) ahead | Fills the voice ring and reports timings. Until it lands, `VirtualVoice` stands in: estimated timings, real-time playback, the same reports |
| **Tools** | blocking pool | | Repo reads and search, bounded (§5) |
| **Harness** | one subprocess per task | JSON lines on stdout | Cancel: SIGINT, then kill after 3 s |
| **Store** | tokio task, the only writer | `StoreSink` port: inputs, UI events, logs and memories (non-blocking), `recall` and `open_session`, `flush` | Event log, vault files, SQLite index. Until it lands, `MemoryStore` keeps this session's memories in memory |
| **Bridge** | tokio task (`runtime::bridge`) | `UiSink` trait (a Tauri channel now, a WebSocket later) | Batches UI events per 16 ms frame; the first step after a quiet frame goes at once. A batch is stamped with the latest step time it carries, so nothing shows before the core produced it. The orb, the thread, the camera of a layer and a caption not yet final coalesce, latest wins. A frame over 256 events sends a full batch early; drawing events are never dropped. |

**Speech in** is anything holding a `SpeechInLink` (voice start, voice end, transcripts). Until the speech stream lands, typed text stands in: `say_typed` gives a line the inputs of a finished utterance, with its voiced length estimated per word, so barge-in rules apply as they would to speech.

**Time.** All runtime time comes through a `Clock` trait, real or fake. The core's `SessionTime` is monotonic time since the session started. Sync with the voice uses the separate playback offset, which comes from the audio clock.

### 3.2 Adapter interfaces

```rust
pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> LlmCapabilities; // mid-conversation system messages, tools…
    fn stream(&self, conversation: &Conversation, cancel: CancelToken)
        -> BoxStream<'static, Result<ChatEvent, ProviderError>>;
}
pub enum ChatEvent {
    TextDelta(String),
    ToolCall(ToolInvocation), // parsed with ToolCall::parse; a call that fails is delivered with `call: Err`
    Done { stop: StopReason, usage: Usage, raw: Option<RawAssistant> },
}

pub trait SpeechToText: Send {
    fn push(&mut self, pcm: &[f32]);
    fn partial(&mut self) -> Result<Option<String>, SttError>;
    fn finish(&mut self) -> Result<String, SttError>;
    fn set_vocabulary(&mut self, terms: &[String]); // repo symbols, recent terms
}

pub trait TextToSpeech: Send + Sync {
    fn synthesize(&self, clause: &Clause, voice: &VoiceId, cancel: CancelToken)
        -> BoxStream<'static, Result<TtsChunk, TtsError>>; // PCM + word timings
}

pub trait CodingHarness: Send + Sync {
    fn run(&self, task: HarnessTask, cancel: CancelToken) -> BoxStream<'static, HarnessEvent>;
}
```

Dynamic dispatch at these seams is fine: it happens per request or per clause, never per audio sample. Errors are hand-written enums that keep their category (`Auth`, `RateLimited { retry_after }`, `Network`, `Refused`, `BadOutput`, …).

The system prompt and tool set are session config (`TalkerConfig { role, model, effort, max_tokens, system_prompt, tools }`), so they never vary per request. Dropping a stream cancels it; `CancelToken` ends it without a drop and asks a harness to stop gracefully. `HarnessPool` wraps any `CodingHarness` with the limits (§3.5).

### 3.3 LLM adapters

**Anthropic** (raw HTTPS and SSE; Rust has no official SDK):

- **Session setup:**
  - The system prompt is frozen for the session and identical across sessions, so it caches.
  - The full tool set is declared once at session start, with `eager_input_streaming: true`. Tool inputs are validated against their schema before running.
- **Every request:**
  - `stream: true`
  - `tool_choice` auto (forced tool choice is rejected by current models)
  - No assistant prefill
- **Stop reasons:** `refusal` becomes a short spoken line, and the talker opts into server-side fallbacks where the model supports them. `max_tokens` finishes the current sentence.
- **Context injection:** the session recap, board and thread updates, and research results are appended as `role: "system"` messages, which keeps the cached prefix intact (supported without a beta on Opus 5.5, Opus 5, Opus 4.8, Fable, Mythos, Sonnet 5.5 and Haiku 5.5). The API accepts one only right after a user turn, and only last or followed by an assistant turn; a run of consecutive context messages becomes one system message. Anywhere else, and on other models, the text is folded into the next user turn as a `<system-reminder>` block (after any tool results, which must lead their turn). The render is a pure function of the conversation, so every assistant turn is echoed after exactly the request that produced it (tested as a property). One edit remains possible: context appended last and then followed directly by a user turn moves into that user turn, which costs one cache miss after the last thinking block and invalidates nothing.
- **History is append-only** (I3); interruptions follow §2.7. Assistant turns are stored exactly as returned, thinking blocks included, and echoed back unchanged.
- **Request shape:** `thinking: {type: "adaptive"}` and `output_config.effort` on models that take them (thinking can't be disabled on Opus 5.5; effort `none` becomes `low` there). A cache breakpoint on the system prompt (tools render before it, so both cache) plus top-level automatic caching for the conversation tail. On models with server-side refusal fallbacks (Opus 5.5, Opus 5, Fable 5.1, Sonnet 5.5) the request carries `fallbacks: "default"` with the `server-side-fallback-2026-07-01` beta; after a mid-stream switch, the echoed content keeps only text before the last `fallback` block, as the API requires.
- **Tool calls** are held until `message_stop`, when the stop reason is known. In a turn that stopped on `max_tokens` or `refusal`, every call is delivered as invalid and never runs; a call whose eager-streamed input is cut off or not valid JSON is delivered invalid too, so the core answers it with an error result.
- **Retries and deadlines:** at most 2 jittered retries, only while nothing has been delivered (a partly delivered stream is never replayed). The first-token deadline covers the retries: 10 s for the talker, 60 s for the scribe. After the first token, 30 s of silence is a timeout. Anthropic pings while the model thinks. Jitter comes from the clock mixed by splitmix64, with no RNG.
- **Defaults:** talker `claude-opus-5-5` at effort `low`; scribe `claude-haiku-5-5` at effort `medium` (set explicitly, since the model's default could change). Spike S3 measures time to first token and may change the recommended talker. That is a product decision recorded in DECISIONS.

**OpenAI-compatible** (`/v1/chat/completions`, streaming): OpenRouter, Ollama (`http://localhost:11434/v1`), LM Studio and similar. Text, tools and stop reasons map to the same `ChatEvent`. Context stays a `role: "system"` message (these servers accept one anywhere). Tool results become `tool` messages, and tool-call argument fragments are joined by `index`. Effort goes out as `reasoning_effort`, and `none` turns thinking off. A local thinking model needs that to talk: at `low`, `qwen3.5:2b` spent a 400-token budget reasoning and never answered; at `none` its first token came in 278 ms. The key is optional.

**Talker tools.** The set is fixed per session:

| Tool | What it does |
|---|---|
| `read_file { path, start_line, end_line }` | Reads from the open repo; bounded |
| `search_code { query, glob }` | Regex search that respects `.gitignore`; bounded |
| `list_dir { path }` | Lists a directory |
| `research { question, depth }` | Starts a harness task and returns at once with a task id. The result arrives later as an appended system message, and the core starts a turn to report it when the conversation is idle. |
| `review { scope }` | A harness review, read-only |
| `propose_change { instruction }` | Puts the core into `AwaitingConfirmation` (§3.5) |
| `remember { kind, text, links }` | Passes through the memory policy |
| `add_task { title, project, links }` | Adds a task |
| `recall { query }` | Searches the index for memories, notes and digests |
| `open_session { id }` | Brings back a past board |

### 3.4 Speech

- **Audio I/O.** The `AUVoiceIO` unit (VoiceProcessingIO), through the objc2 bindings (`objc2-audio-toolbox`, `objc2-core-audio`, `objc2-core-audio-types`), gives echo cancellation, noise suppression and gain control. All `unsafe` lives in `runtime::audio::coreaudio`.
  - **Rates.** The unit only initializes at its own client rate (44.1 kHz on a MacBook Pro running macOS 26; asking for 16, 22.05, 24 or 48 kHz fails with `kAudioUnitErr_FailedInitialization`). It runs mono `f32` at that rate, and an allocation-free polyphase windowed-sinc converter sits at each ring boundary: capture to the 16 kHz mic ring, the 24 kHz voice ring to the unit.
  - **Rings.** The mic ring (2 s) overwrites its oldest frames when speech in falls behind and counts them; it is a seqlock over atomics, because `rtrb` can only refuse new items. The voice ring (4 s) is `rtrb`. A counting-allocator test proves the callbacks never allocate (law 7).
  - **Playback clock.** The renderer counts frames taken from the voice ring (never silence or dropped frames). `PlaybackAt` = that counter minus the output latency (device latency + safety offset + IO buffer + stream latency + the unit's latency + the converter's delay; 27.6 ms on the built-in speakers). When the ring runs dry, the heard position follows the clock for one latency so `PlaybackDrained` comes when the last frame is heard.
  - **Duck and stop.** The duck gain ramps per sample over 10 ms. `StopVoice` fades out over 30 ms, drops the rest of the ring, and answers with the counter at the end of the fade: every frame up to it reaches the speaker, nothing after it does.
  - **Other apps.** Ducking of other apps' audio is set to the minimum (`kAUVoiceIOProperty_OtherAudioDuckingConfiguration`, macOS 14+).
  - **Input.** The system default input, or the Mac's built-in microphone when the default is a virtual device that hears nothing. Without microphone permission macOS delivers silence, not an error.
- **Voice activity detection.** Silero v5.1.2 on ONNX Runtime (`ort`), 512-sample windows with the model's recurrent state and 64 samples of context carried between windows: speech at or above 0.5, silence below 0.35 (in between keeps the state), a 150 ms minimum, 250 ms hangover. whisper.cpp's built-in Silero was rejected: it resets the recurrent state on every call, so it cannot follow a stream in 32 ms steps.
- **Speech-to-text.** whisper.cpp through `whisper-rs`, using Metal.
  - Two dedicated threads: the VAD thread (so barge-in never waits behind a decode) and the speech-to-text thread. `VoiceStart`/`VoiceEnd` reach the orchestrator before the speech-to-text thread hears of them, so a transcript never precedes its `VoiceEnd`.
  - The default model is `base.en`; `small.en` is optional (its decodes are 2–3× slower; see S4).
  - A partial is decoded on a fixed 500 ms grid from the onset, over the current utterance only; a slot that comes while a decode is busy is skipped. The utterance includes 300 ms of audio before the onset.
  - **Early final.** After 96 ms of quiet inside speech the final is decoded at once; speech that resumes discards it. The hangover (256 ms) usually hides the whole decode, so the final arrives with `VoiceEnd` (spike S4: 0–1 ms after it on `base.en`).
  - The encoder always sees its full 30 s window. Fitting it to the utterance (`audio_ctx`) was measured and rejected: repetition loops and slower decodes.
  - Vocabulary goes in as Whisper's initial prompt: repo symbols and recent terms, whole terms only, capped at 200 tokens.
- **Text-to-speech.** Engines return 24 kHz audio and word timings as byte offsets into the clause's speakable text. A clause an engine fails on is retried with the system voice; if that fails too it is skipped (logged), since the core has no input for a failed clause yet.
  - **Kokoro** (82M v1.0, ONNX, Apache-2.0) is the local default, voice `af_heart`, on one inference thread. The export used (`onnx-community/Kokoro-82M-v1.0-ONNX-timestamped`) returns each token's predicted duration (600 samples per frame), so word timings are exact. The model gives the leading pad token ~350 ms of silence; all but 50 ms is trimmed so clauses join without a stammer. Clauses over 510 phoneme tokens are split at a word.
  - **English pronunciation without espeak-ng** (GPL): misaki's `us_gold` lexicon (Apache-2.0), then what misaki derives from it (possessives, plurals, past tenses, gerunds), unknown acronyms spelled out, camelCase and compound words split into known words, numbers read as words (a short function; `num2words` is LGPL), and table-driven letter-to-sound rules last. misaki's `us_silver` is left out: its author generated it by running espeak-ng.
  - **ElevenLabs** uses `POST /v1/text-to-speech/{voice}/stream/with-timestamps?output_format=pcm_24000`: newline-delimited JSON with base64 16-bit PCM and the original text's character alignment, which becomes exact word timings (estimated if the alignment does not spell the clause).
  - **macOS system voice:** `AVSpeechSynthesizer` writing to buffers with word markers (macOS 14+; 22.05 kHz, converted). Exact timings, no download. Its callbacks run on the main dispatch queue whatever thread asks, so the process's main thread must run an event loop (the app's does; headless binaries call `audio::run_main_loop_until`). It is used during first-run model downloads and whenever the others fail.
- **Model files** download to `~/Library/Application Support/ai.boardai.desktop/models` on demand, from URLs pinned to a commit. Downloads resume with HTTP `Range` (a server that ignores it restarts the file), are SHA-256 verified before they are renamed into place, report progress, and show their size before downloading. Nothing is bundled. `just models` shows and fetches them.

### 3.5 Coding harness adapters

| | Claude Code | pi |
|---|---|---|
| **Process** | `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --session-id <uuid> --append-system-prompt <brief>`, run in the repo | `pi --mode rpc` (strict LF-delimited JSON lines), run in the repo |
| **Research / review** | `--permission-mode plan`, `--allowedTools Read Grep Glob` | `--tools read,grep,find,ls` |
| **Change** | `--permission-mode acceptEdits`, on branch `boardai/<slug>` | Default tools, on branch `boardai/<slug>` |
| **Output we read** | init, assistant text and tool use, tool results, final result | The RPC event stream |

- **Common output.** Both map to `HarnessEvent`: `Progress(text)`, `FileTouched(path)`, `Finished { summary, citations }` or `Failed`.
- **Confirmation gate (I9).** `propose_change` makes BoardAI say what will happen and on which branch. The core accepts the change only if the **next user utterance** matches the affirmative lexicon (yes, go, go ahead, do it, sure, okay). A tool result or repo content can never confirm a change, which blocks prompt injection from files.
- **Limits.**
  - Concurrency: 2 research or review tasks and 1 change task at a time; up to 4 more queue, and beyond that a task is refused out loud.
  - Timeouts: 10 minutes for research, 30 for a change.
  - Captured output is bounded to the last 256 KB.
- **Discovery.** The harnesses are found on `PATH` at startup, then in `~/.local/bin`, `~/.npm-global/bin`, `/opt/homebrew/bin` and `/usr/local/bin`: an app opened from Finder gets a minimal `PATH`. Settings choose the default.
- **Claude Code details** (flags checked against `claude --help`, 2.1.301). We are the "host" of a stream-json session and answer no permission prompts, so every run also passes `--permission-prompts none`: anything that would prompt fails instead of hanging. `--append-system-prompt` carries BoardAI's fixed brief for the task kind (read-only, or edit this branch only, and cite `path:line`). The task's own brief goes in as one stream-json user message, and stdin closes after the `result`. Multi-value flags get one comma-separated argument (`--allowedTools Read,Grep,Glob`).
- **pi details** (`docs/rpc.md`, pi 1.3.0). One `prompt` command; the run ends at `agent_settled`. stdin closes only after that, because closing it mid-run aborts pi. An older pi without `agent_settled` finishes on `agent_end` with no retry pending. `--no-extensions` keeps extension dialogs away, and any dialog that still arrives is declined.
- **Change preflight.** The branch must be `boardai/<slug>`. A dirty tree is refused, and untracked files count, so the harness's new files are its own. Then `git switch --create` runs.
- **Lifecycle.** Each process runs in its own process group. Cancel is SIGINT to the group, then SIGKILL after 3 s, sent through `/bin/kill`: the runtime has no `unsafe`, and an unreaped child's id can't be reused. A finished task gets stdin closed and the same grace before it is interrupted. The terminal event is delivered only after the process is reaped. Dropping a stream kills the group. Citations are kept only for files that exist inside the repo.

### 3.6 Store (panel F)

```
~/BoardAI/                          the vault (source of truth, yours)
  sessions/2026-10-14-request-path/ <date>-<time> while recording, renamed after the digest's title
    events.jsonl                    append-only session log
    board.excalidraw                final board, one frame per layer (the core's export)
    digest.md                       summary, diagrams, open questions, captured items, proposals, graph
  notes/<slug>.md                   one note per file; front matter: id, created, session, links
  projects/<name>.md                tasks as checklist lines with ids: - [ ] Check Redis TTLs [[session:…]] ^t27
  memory/about-you.md               preferences and learner model, editable by hand: - … ^m12
  memory/facts.md                   facts about your systems
```

- **The actor.** One tokio task owns every write, fed by one bounded inbox (4096). Blocking work (files, fsync, SQLite) runs on the blocking pool; the state moves into the blocking closure and back, so it has one owner and no lock. Shutdown flushes and closes a session still recording.
  - Log entries never wait: the handle `try_send`s them, and a full inbox drops and counts them (a `dropped` line records the count). The orchestrator's latency matters more than a complete log, and a full inbox means the disk is stalled.
  - Requests (start, end, remember, forget…) wait for room: their callers await a reply anyway.
  - Reads (recall, library, Now, open session, recap) start in inbox order, so they see every earlier write, but run on a separate read-only connection (WAL) beside the actor, so a query never delays a flush.
- **Redaction (I8).** Every write goes through `redact_text` or `redact_json_text` (string literals only, so the JSON shape and field order are unchanged), and writers take only a `Redacted`. A table of rules: API keys (`sk-ant-`, `sk-or-`, `sk-`, ElevenLabs `sk_`, Stripe, Google, Slack, GitHub), AWS key ids and secret assignments, PEM private keys (across a code card's lines too), JWTs, URL passwords, `Authorization`/`Bearer`/`x-api-key` headers, and `password=`-style assignments. Text you paste into your own files is redacted on its way into the index; your file is left as you wrote it.
- **Event log.**
  - One JSON record per line: `{"v":1,"t":1234,"k":…}`. Kinds: `session` (first line: id, start), `ui` (exactly `UiLogLine`, which the UI replays), `input` (`{"input":…}`, a core `Input`, for re-simulation with the same build), `record` (a core `LogRecord`), `dropped` (reason and count) and `end`.
  - Flushed every 250 ms (one `write`, no fsync) and at session end (fsynced), so a crash loses at most 250 ms. A line over 1 MiB (the UI's own limit) is replaced by a counted `dropped` marker, never cut mid-JSON. The reader streams, and skips and counts malformed, oversized and other-version lines.
- **Sessions.** A session's id is `<date>-<time>` (UTC), kept in the log's first line and the digest's front matter, so renaming the folder after the title breaks no link. At the end the log is closed, the scribe writes the digest from the transcript (final captions), and `digest.md` and `board.excalidraw` are written. A session that ends without one (the app quit, the next session started) gets the minimal digest.
- **Scribe.** Asks for one fixed JSON shape (title, summary, diagrams, open questions, proposed memories, entities, relations), parsed strictly with bounded lists. A provider failure, timeout (120 s), refusal or junk gives the minimal digest: a title (hint or first question), a transcript-derived summary line, and the captured items. It never creates tasks or kept memories.
- **Memory writes.** The policy's action is carried out per kind: stated and learning memories to `about-you.md` or `facts.md` by subject, notes to `notes/`, tasks to `projects/` (`inbox` without a project), session records into the digest; `Propose` waits in the digest until `accept_proposal`. Raw audio, secrets and `Never` are refused. Each write is a capture: "forget that" undoes the last; "don't remember this session" (I10) undoes all of them, removes files the session created, deletes the session folder and its index rows, and drops the rest of the session.
- **Vault writes.** Write a temporary file, fsync, rename, fsync the folder. Slugs are lowercase ASCII words, at most 60 bytes, with `-2`, `-3`… on collision. The vault path is validated at startup (absolute, a folder, not `/` or your home folder, writable), and temporary files a crash left are removed.
- **Index.** `index.sqlite` lives in the app data folder; bundled SQLite with FTS5.
  - Tables for sessions, notes, tasks, memories, proposals, entities and relations (`mentioned_in` added for every entity of a digest), plus full-text search over notes, digests, memories, tasks and transcripts.
  - A `files` table holds each file's modification time, size and hash (FNV-1a); every row names its file, and a changed file re-indexes its unit (a session folder, a note, a project, a memory file).
  - A full rebuild is one scan of the vault. The schema is versioned in `user_version` with ordered migrations; an index from a newer build, or a damaged file, is deleted and rebuilt.
- **Replay audio.** BoardAI's voice will be stored as Opus in Phase 1. Your voice is never stored; replays show your words as captions.

### 3.7 Protocol (core ↔ UI)

- **Events (core → UI):**
  - `Orb { state, level }`, `Caption { who, text, is_final }`, `Thread { steps, current, detours }`
  - `OpStart { layer, op, budget_ms }`, `Freeze { layer }`, `Resume { layer, op, budget_ms }`
  - `Layer { push | pop, title }`, `Camera { focus }`
  - `Surface { show | update | dismiss }`, `Chip { kind, text }`
  - `Library { … }`, `Brain { … }`, `Problem { spoken, detail }`
- **Commands (UI → core):** `Pointer { target }`, `Mic { mute | unmute | talk_down | talk_up }`, `OpenLibrary`, `OpenBrain`, `ReplaySeek { ms }`, `SaveSettings { … }`.
- **Types** are defined in `core::protocol` and exported to `ui/src/protocol/generated/` by ts-rs. A stale export fails `just check`.
- **Transport.** A Tauri `Channel` carries events in batches per frame: `connect` registers it, and batches made before it (the session starts before the webview loads) wait, at most 64, and go first. One `invoke("command", …)` carries commands; a full inbox drops the command and the UI counts the rejection. The app's capabilities allow only these two commands; debug builds also grant `dev_say(text)`, a typed utterance for the dev overlay, which release builds don't have. The UI only sees a `Transport` interface, with Tauri, replay-file and (later) WebSocket implementations.

### 3.8 Settings and secrets

- **Settings** live in `settings.toml` in the app's config folder:
  - providers per role (kind, model, base URL)
  - voice
  - vault path
  - code roots, which are where "open the boardai repo" looks
  - mic mode
- **Shape.** Top-level `vault`, `code_roots` (absolute or `~/`) and `mic` (`open` or `push_to_talk`), plus a table per role:

  ```toml
  [talker]   # and [scribe]: kind = "anthropic" | "openai_compatible", model, effort (none…max), max_tokens, base_url, secret
  kind = "anthropic"
  model = "claude-opus-5-5"
  effort = "low"
  secret = "anthropic"          # a name, never the key
  [stt]      # kind = "whisper", model
  [tts]      # kind = "kokoro" | "elevenlabs" | "system", model; base_url and secret for elevenlabs only
  [harness]  # kind = "auto" | "claude_code" | "pi", model
  [voice]    # id
  ```

  Everything has a default (the D-017 models, Ollama's URL for `openai_compatible`). Unknown keys are errors. Errors name the field and never quote a value: a value that looks like a key, or a misplaced one, is rejected without being echoed.
- **Secrets** are stored in the Keychain (`keyring`) under names that the settings reference, read through the runtime's `SecretStore` trait. A secret is never put in settings, logs or the event log; `Secret` has no `Display` and a redacted `Debug`.
- **First run.** The first run is the one screen that needs typing: choose providers, paste keys, watch the model downloads, and grant mic permission. Its design is artboard 11 on the design canvas.

## 4. UI

- **Structure.** One stage. Preact components for the surfaces and views, signals for captions and the thread.
- **Renderers.** The canvas and orb renderers are plain imperative TypeScript on `requestAnimationFrame`, writing SVG directly. There is no virtual DOM on the hot path.
- **Canvas.** The stroke engine is ported from the design prototype:
  - generators for rough lines, ellipses, arrowheads and hachure fills
  - stroke reveal with human pacing (slower into corners)
  - handwriting revealed letter by letter

  Randomness is seeded by element id, so a replay draws identical strokes. Handwriting sits above every stroke on its layer and fills sit under the ink, as on the canvas.
- **Pacing.** An op draws at the design's natural pace, including the pen's travel between items. When that would overrun `budget_ms`, the whole op is compressed to fit; it is never stretched, so the nib rests between ops instead.
- **Freeze and resume.** The UI owns each stroke's progress. `Freeze` stops it in place, and `Resume` continues from there, fitting what is left into its new budget. The nib waits on the frozen point, pulsing, until then.
- **Orb.** A port of the design's orb component: five blurred gradient blobs, plus the listening dot field. Its state targets are eased exponentially from the last change, evaluated in closed form so a seek lands on the same frame. `Freeze` plays the interrupted contraction and `Acknowledge` a small pulse. It pauses when the window is hidden.
- **Animation clock.** Every animated value is a function of session time. Live, the clock follows the session time stamped on each event batch: it jumps forward when a batch is ahead of it and never runs backwards, so strokes start where the core scheduled them. In replay, the playhead is the clock.
- **Transport.** The Tauri transport calls `invoke("connect", { events: Channel<UiBatch> })` once, then `invoke("command", { command })` per `UiCommand`. Batches are parsed again at this boundary; malformed events are dropped and counted.
- **Replay mode.** `?replay=<url>` plays an `events.jsonl` with no backend; `&t=<ms>` opens it paused at that time (for screenshots), `&speed=` and `&controls=0` are options. Outside Tauri with no `?replay`, the demo plays. Malformed lines are skipped and counted. It drives UI development and side-by-side checks against the design canvas. The fixtures in `ui/public/fixtures/` are written by `ui/src/replay/` and a test fails while one is stale: `demo.events.jsonl` is the canvas's frames 1→7, and `primitives.events.jsonl` covers the remaining ops and surfaces.
- **Tokens and fonts.** TypeScript reads its numeric tokens from `tokens.css` at boot and fails on a missing one. Geist, Geist Mono and Excalifont are bundled (latin subsets, OFL), since the CSP allows no remote fonts.
- **Budgets.** At most 4 ms of script per frame. One SVG path per stroke and one per hachure fill.

## 5. Limits

Every bound is a named constant in its crate's `limits` module, documenting what happens when it is crossed. A limit that exists only as a literal is a defect.

| Limit | Value | Owner | When crossed |
|---|---|---|---|
| Mic ring | 2 s | runtime::audio | Oldest frames are dropped and counted. The real-time thread never blocks. |
| Voice ring | 4 s | runtime::audio | Synthesized audio waits in speech out until playback frees room |
| Real-time cycle | 8,192 frames | runtime::audio | A larger cycle gets silence and is counted; buffers are never grown on the real-time thread |
| Utterance | 30 s | core | The turn ends |
| Talker streams | 1 | core | A new request cancels the old one |
| Speech lookahead | 4 clauses / 15 s | runtime::speech_out | Synthesis waits for playback |
| Playback reports | one per 20 ms | runtime::speech_out | Latest wins (`watch`) |
| Stop acknowledgement | 250 ms | runtime::speech_out | `VoiceStopped` is answered from the counter; new audio waits for the device |
| Clause synthesis | 15 s, 40 s of audio | runtime::speech_out | Fails with `Timeout` and the system voice is tried; audio past 40 s is cut and counted |
| Kokoro sequence | 510 tokens | runtime::speech_out | The clause is synthesized in parts, split at a word |
| ElevenLabs response | 16 MB, 4 MB per line | runtime::speech_out | The clause fails with `BadOutput` |
| Speech-to-text audio | 30 s | runtime::speech_in | The rest is dropped and counted (the core ends the turn at the same length) |
| Speech-to-text queue | 1,024 windows (~32 s) | runtime::speech_in | A window is dropped and counted |
| Speech-in inputs | 64 | runtime::speech_in | A partial transcript is dropped and counted; voice start, end and finals wait |
| Vocabulary prompt | 200 tokens | runtime::speech_in | Later terms are left out |
| Model download silence | 30 s | runtime::models | Fails with `Network`; the partial file is kept for resuming |
| Narration tag | 512 bytes | core::narration | The tag is dropped and counted |
| Ops per response | 200 | core | The rest are dropped, and the talker is told in the next turn |
| Detour depth | 3 | core | A spoken "let's finish this one first" |
| Tool result | 400 lines / 64 KB | runtime::tools | Truncated, with an explicit "(N more lines)" marker |
| Harness tasks | 2 read + 1 change, queue 4 | runtime::harness | Refused out loud |
| Harness time | 10 min / 30 min | runtime::harness | Cancelled, with a spoken notice |
| Provider retries | 2, jittered; 10 s deadline for the talker | runtime::llm | A spoken failure line plus a `Problem` event |
| UI events per frame | 256 | runtime::bridge | Captions and levels coalesce |
| Event log flush | 250 ms | runtime::store | A crash loses at most this much; earlier at 8192 buffered entries |
| Store inbox | 4096 messages | runtime::store | Log entries are dropped and counted in a `dropped` line; requests wait |
| Event log line | 1 MiB | runtime::store | Replaced by a counted `dropped` marker; the reader skips and counts longer lines |
| Vault file read | 8 MiB | runtime::store | Not indexed; never edited by the store |
| Captures per session | 500 | runtime::store | `remember` fails with `Limit` |
| Recall answer | 8 hits / 4 KB | runtime::store | The rest are counted as "N more" |
| Library / Now | 500 sessions / 12 tasks | runtime::store | The rest are counted |
| Scribe | 48 KB transcript, 64 KB answer, 120 s | runtime::store | Transcript keeps its start and end; past the others, the minimal digest |

## 6. Errors

- **Operational errors are values.** Network, auth, a provider refusal, a missing model or a bad tool input never panic. A panic means a broken invariant.
- **The user hears the failure.** A failure becomes a short spoken line plus a caption, for example: "I lost the connection to Claude. Want me to switch to the local model?" The detail goes to the log.
- **Errors keep their meaning.** A timeout is never reported as success, and a refusal is never reported as "no answer".

## 7. Testing

| Layer | How |
|---|---|
| Core | Unit and table tests, plus the **conversation simulator**: scripted timelines of inputs on a fake clock, asserting effects and UI events. For example, "mm-hm" at 3.2 s never stops the voice, and an interruption at word 9 commits exactly 9 words. A seeded fuzzer explores input interleavings and checks I1–I10 with no panics. |
| Narration parser | Golden files, the split-invariance property, and random bytes (never panics, bounded memory) |
| Runtime adapters | Recorded fixtures: SSE transcripts, stream-json and RPC logs, short WAV clips. CI uses no network. `just live` runs real-provider smoke tests with your keys, never in CI. |
| UI | `node --test` on pure TypeScript modules (stroke generators are deterministic per seed; clock math), plus visual review in replay mode against the design |
| End to end (Phase 1) | The app with a WAV file as the mic and fake providers, asserting `events.jsonl` |

`just check` runs:

- Rust: fmt, clippy with `-D warnings`, tests, `cargo deny`
- UI: biome, typecheck, `node --test`
- The protocol freshness check and the file-length check

CI runs the same command on macOS.

## 8. Observability

- The core's `Log` milestones become `tracing` events (target `boardai::latency`) stamped with session time and a turn number: voice start, final transcript, talker requested, talker first token, first audio, first op, barge-in duck and barge-in stop. Turns are grouped by that number rather than spans.
- Logs go to stderr through `runtime::logging`, a small subscriber that bounds every field (512 bytes) and line (4 KB) and redacts key-shaped text on the sink (`sk-…`, `ghp_…`, AWS keys, PEM blocks, bearer tokens, `password=`).
- `just converse` prints the marks per answer, with ops drawn and narration tags dropped, for spikes S3 and S5.
- A dev overlay (toggle with `⌘⇧L`) shows live latencies. Each session's digest records a latency summary locally.
- Logs never contain secrets or full transcripts. Fields are bounded, and redaction runs on the log sink.

## 9. Security

- **API keys** live in the Keychain only, and the redaction filter covers every sink.
- **Repo access.** Every path is canonicalized into a `RepoPath` at the boundary and must sit under a folder you opened. There is no access outside it.
- **Prompt injection.** Repo content, docs and tool results are data. Nothing they say can trigger a change (I9). The system prompt says so, and the confirmation gate enforces it.
- **Webview.** A strict CSP, no remote content, and minimal Tauri capabilities (the event channel and one command).

## 10. Dependencies

New entries need a row in [DECISIONS.md](../DECISIONS.md).

| Where | Crates or packages |
|---|---|
| core | `serde`, `ts-rs` |
| runtime | `tokio`, `reqwest` (rustls with ring), `rustls`, `futures-util`, `serde`/`serde_json`, `toml`, `ignore`, `regex`, `tracing` (D-020); `rusqlite` bundled (D-039); objc2 + CoreAudio/AVFAudio bindings and `rtrb` (audio leaf, D-054); `ort` (D-055); `whisper-rs` (D-056) |
| app | `tauri`, `tauri-build`, `keyring-core` + `apple-native-keyring-store` (D-045); logging is the runtime's own subscriber (D-046) |
| ui | `preact`, `@preact/signals`, `@tauri-apps/api`; dev only: `vite`, `typescript`, `@types/node`, `@biomejs/biome`, `@tauri-apps/cli` |

## 11. Phase 0 build order

1. **Core skeleton.** `SessionTime`, inputs and effects, the state machine with the cue tables, and the simulator with its first scenarios.
2. **Narration.** The parser, speakable text, clauses, the op scheduler, and board layout. All pure and fully tested.
3. **UI.** Port the orb and stroke engine from the design, and build replay mode. The design demo as a fixture must look like the canvas.
4. **Talker.** The Anthropic and OpenAI-compatible adapters, plus `just converse`, a text-only dev loop that prints the narration and ops (no audio).
5. **Audio.** VoiceProcessingIO, Silero VAD and Whisper (spikes S1 and S4).
6. **Voice.** The system voice first, then Kokoro and ElevenLabs (spike S2). The playback clock drives ops.
7. **Steel thread.** Speak → draw → interrupt → resume, end to end. Exit when the product's Phase 0 criteria are met.
