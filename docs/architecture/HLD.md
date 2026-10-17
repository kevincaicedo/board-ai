# BoardAI · High-level design

> Status: **draft for review** · 2026-10-15 · Diagram: [`hld.excalidraw`](hld.excalidraw) (open it at [excalidraw.com](https://excalidraw.com) → menu → Open) · Low level: [LLD.md](LLD.md) · Decisions: [../DECISIONS.md](../DECISIONS.md)

## 1. What this document covers

BoardAI is a voice-first desktop app. You talk, it explains while drawing hand-drawn diagrams in sync with its voice, you interrupt, and it resumes exactly where it stopped. It remembers what you learned and can hand deep work to a coding harness. The product is in [PRODUCT.md](../PRODUCT.md); the look is in the [design canvas](https://claude.ai/artifact/XY3ZrPFmFDA9gNw9Y6WuVJ).

This document fixes the shape of the system: the parts, what each part owns, how they talk, where data lives and what we prove first. The [LLD](LLD.md) fills in the types, algorithms and limits.

## 2. What the architecture must get right

These are in priority order. The order settles arguments.

1. **Correct conversation.** BoardAI never claims what it didn't read. It never loses what you said. After an interruption, it knows exactly which words you heard and which strokes were drawn.
2. **Latency that feels human.** See the budget in §8. Every part of the pipeline streams.
3. **Private by default.** Everything is stored on your machine as plain files. Data leaves only for the providers you configured, and with local models nothing leaves at all.
4. **Fast iteration.** We can develop and test the conversation without speaking, without audio hardware and without paying for API calls.
5. **Swappable parts.** Every provider (LLM, speech-to-text, text-to-speech, coding harness) sits behind one interface. The same core can later run in the cloud for the hosted version.

**Not goals for the MVP:** Windows and Linux, multiple users, cloud sync, plugins, a text box.

## 3. The one rule

**The core decides, the runtime does, the UI shows.**

| Part | Technology | Owns | Never does |
|---|---|---|---|
| **Core** | Pure Rust library | The conversation state machine, the narration parser, speech-to-stroke scheduling, the board model and layout, the thread and detours, and the memory policy | Any I/O. No clock, no randomness, no threads, no async. |
| **Runtime** | Rust with tokio | All I/O: audio, speech engines, LLM providers, coding harness processes, repo reads, files, SQLite, the event log. It also feeds time and inputs into the core. | Conversation decisions |
| **App shell** | Tauri 2 | The window, OS permissions, keychain access and the transport between UI and runtime | Business logic |
| **UI** | Preact and TypeScript in the webview | Drawing the orb, strokes, surfaces and captions. It reports what you point at. | Decisions, I/O, or timers that change state |

This split pays for itself in three ways:

- **Deterministic tests.** The core is a function: given the same inputs at the same times, it produces the same effects. We test whole conversations, including interruptions, in milliseconds and with no audio, network or API cost.
- **Replay for free.** The runtime logs every input. Replaying the log rebuilds the session exactly, which gives us the replay feature, test fixtures and bug reproductions.
- **Hosted later without a rewrite.** Core and runtime have no Tauri dependency. The UI talks to the runtime only through a typed protocol, so the same code can run behind a WebSocket.

## 4. Components

The [diagram](hld.excalidraw) shows these boxes and how they connect.

**Core** (pure):

- **Conversation state machine.** Idle, Listening, Thinking, Speaking, Ducked and Muted, plus turn-taking and the backchannel-versus-interruption decision.
- **Narration parser.** Reads the LLM's streamed output: spoken text interleaved with drawing tags such as `<node id="lb" …/>`. It is tolerant of malformed output and never blocks speech.
- **Op scheduler.** Fires each drawing op when the voice actually reaches the word the op is anchored to.
- **Board model and layout.** Turns meaning ("to the right of the CDN") into geometry. Existing elements never move.
- **Thread and detours.** The explanation plan, plus a stack of detours opened by interruptions.
- **Memory policy.** Decides what is remembered automatically, what is proposed, and what is never kept.

**Runtime** (I/O):

- **Audio I/O.** macOS VoiceProcessingIO provides echo cancellation, so an open mic doesn't hear BoardAI's own voice.
- **Speech in.** Voice activity detection plus speech-to-text, producing partial and final transcripts.
- **Speech out.** Text-to-speech plus playback. The audio sample counter is the session clock.
- **Talker client.** Streams from the LLM and runs its tools.
- **Repo tools.** Native, read-only file reading and search in the opened repo.
- **Harness runner.** Runs Claude Code or pi as a subprocess for deep research, reviews and confirmed changes.
- **Store.** The event log, the vault files and the SQLite index. It is the only writer.
- **Scribe.** Background memory work: the session digest, extracting entities, proposing memories.

**UI:**

- The orb, the canvas stroke renderer, surfaces (code card, compare, doc), captions and the thread bar, the library and the brain views.
- The first-run screen, and the settings page (⌘, or "open settings"): the talker and its model picked from the providers' lists, the voice, the microphone and its permission, the keys (saved, replaced or removed in the Keychain, never shown), the coding harnesses and code folders, and what memory keeps.
- A replay mode that plays a recorded session with no backend. UI work is checked against the design canvas this way.

## 5. Key flows

The numbers match the badges on the diagram.

1. **Hear.** Mic → echo cancellation → voice activity detection → speech-to-text partials → core. End of turn is decided from silence plus whether the sentence sounds finished.
2. **Think.** The core asks the talker for a response. The request carries the transcript, a board summary, the thread, and relevant memory. The LLM streams back narration.
3. **Speak and draw.** The parser splits the narration into spoken clauses and drawing ops anchored to words. Clauses go to text-to-speech. Playback reports its position, and the core fires each op as the voice reaches its word. The UI animates the stroke within the time budget until the next op.
4. **Interrupt.** Your voice starts, so BoardAI's volume ducks within 100 ms. The core then classifies what you said:
   - A backchannel ("mm-hm") restores the volume.
   - A real interruption stops the voice and freezes the stroke in place. The core records exactly what you heard, opens a detour, and later resumes from the frozen stroke.
5. **Ground and delegate.** Quick reads use the native repo tools, in milliseconds. Deep research, reviews and changes go to the coding harness, which takes seconds to minutes. BoardAI narrates while the harness works. Results come back with `file:line` sources.
6. **Remember.**
   - Every input is appended to the session's event log.
   - At the end, the scribe writes the digest and proposed memories into the vault, and the index is updated.
   - The next session starts with a short "where we left off" recap.

## 6. Providers: bring your own keys

| Role | Interface | MVP adapters | Default |
|---|---|---|---|
| Talker LLM (the voice) | `LlmProvider` | Anthropic Messages API; Ollama's native chat API (any Ollama, found by asking it, so it can be given a large enough context); OpenAI-compatible (OpenRouter with a provider route and its reasoning setting, any compatible server) | Chosen at onboarding. OpenRouter offers two measured presets routed to Cerebras: gpt-oss-120b (thinking at its least, `low`) and Qwen3.8 27B (thinking off) |
| Scribe LLM (memory, digests) | `LlmProvider` | Same as the talker | A cheaper or local model |
| Speech-to-text | `SpeechToText` | whisper.cpp (local, Metal) | Local Whisper |
| Text-to-speech | `TextToSpeech` | Kokoro (local); ElevenLabs (cloud); the macOS system voice as a zero-download fallback | Local Kokoro |
| Coding harness | `CodingHarness` | Claude Code CLI in headless mode (stream-json); pi in RPC mode | Whichever is installed |

- API keys live in the macOS Keychain. They are never stored in files or logs. The app never reads them from the environment; only the dev tools do, from a gitignored `.env`.
- Each provider's models can be listed for a picker (OpenRouter's tool-capable models with context and price, who serves one of them, Ollama's local models); the lists are public, bounded and cached.
- A session readies its talker when it opens (a local model loads and reads its prompt), so the first answer doesn't wait for it.
- Local models are downloaded on first run and checked against a SHA-256 hash. The macOS system voice covers the wait until the download finishes.
- A cloud speech-to-text adapter (for example Deepgram) is a Phase 1 addition behind the same interface.
- The Claude Agent SDK would need a Node sidecar. The CLI's headless mode gives the same capabilities, so the SDK is deferred.

## 7. Data and privacy (local-first)

- **The vault is the source of truth.** It's a folder you own, `~/BoardAI` by default:
  - Notes and project task lists as Markdown
  - One folder per session, holding the event log (JSONL), the board (`.excalidraw`) and the digest
  - Your memory files
- **The index is derived.** SQLite with full-text search lives in the app's data folder and can always be rebuilt by scanning the vault. The MVP has no vector database.
- **What is kept automatically.** The memory policy in [PRODUCT §7.4](../PRODUCT.md) applies, in short:
  - The session record and learning signals are kept automatically.
  - Anything inferred is proposed in the digest first.
  - Notes and tasks are kept only when you ask.
  - Raw voice audio and secrets are never kept.
- **What leaves your machine.** Only requests to the providers you configured. With Ollama, local Whisper and Kokoro, nothing leaves.

## 8. Latency budget

| Stage | Target | gemma4, local (M4 Pro) | gpt-oss-120b, OpenRouter → Cerebras | Qwen3.8 27B, OpenRouter |
|---|---|---|---|---|
| End-of-turn detection | 200–300 ms | 446 ms p50 (411–448 when the final sounds finished, 645–656 unpunctuated). The floor is Silero's tail after the sound (p50 82 ms) plus the 256 ms hangover | 436–447 ms (633 unpunctuated): the talker doesn't change it | (as for the others) |
| Final transcript after end of turn (partials stream before it) | ≤ 150 ms | 0–1 ms (decoded during the hangover) | 0–1 ms | – |
| First token from the talker | 300–450 ms (measured in spike S3) | 193–385 ms on a follow-up; **~4.0 s on a session's first question** (3,963 ms): gemma4's sliding-window cache can't reuse the system prompt's prefix, so it is prefilled again | 426–460 ms; 509 ms p50 over 11 `converse` answers (375–943); 399 ms p50 after the session warm-up. A list-the-repo tool round adds ~450 ms when it happens | Not measured on Cerebras with BoardAI's prompt beyond 3 requests (461–1,023 ms): the development account's routing sent every such request to Tenstorrent (p50 1,235 ms, 689–3,545) |
| First audio from text-to-speech | 100–200 ms | 196–575 ms from the first token to the first clause, 38–198 ms to synthesize it (system voice) | 0–28 ms to the first clause (the text arrives whole), 54–169 ms to synthesize it | – |
| **Voice to voice** | **p50 ≤ 1.0 s · p90 ≤ 1.5 s** | **p50 1.17 s** over 9 turns (0.93–1.44 s); a session's first question 4.9 s | **p50 1.13 s** over 4 turns (1.02–1.55 s): not yet within budget | Not measured |
| First stroke after the first spoken word | ≤ 200 ms | Not measured separately; each stroke lags its own word by 11–28 ms (p50) | 13 ms p50 | – |
| Barge-in: duck after you start speaking | ≤ 100 ms | 28–86 ms | 60–68 ms | – |
| Barge-in: decision to stop fully | ≤ 400 ms | **331–374 ms** for "Wait, why Redis first?" (was 465 ms before Whisper's early spellings of the cues counted, D-159) | 344–425 ms | – |
| Drift between speech and strokes | ≤ 150 ms | 11–28 ms p50, at most 148 ms | 13 ms p50, at most 30 ms | – |

The runtime measures every stage, and the numbers show in a dev overlay and in each session's digest. The measured columns are `just e2e` (LLD §7: `say` into a file mic, the real VAD, Whisper `base.en` on Metal, the core, the talker, the system voice; three lines with a barge-in), 2026-10-16: gemma4 on a local Ollama through its native API, three runs; gpt-oss-120b on OpenRouter routed to Cerebras at effort `low`, one full run and the first line of another (the development account then ran out of credit and OpenRouter refused the rest). The first-token ranges add `just converse` and `just live` (LLD §3.3). Repeated e2e runs ask the same first question, which Ollama finds cached; a new question doesn't, so the first question of a session on gemma4 waits ~4 s. What is left: end of turn (~440 ms against 300; a speculative start on the early final was measured and rejected, D-155) and, locally, gemma4's first question; on Cerebras the first token is within ~50 ms of the S3 bar.

## 9. Deployment

- **MVP.** A Tauri app for macOS 14+ on Apple silicon. Builds are unsigned during Phase 0, then signed and notarized before the first outside users.
- **Hosted (Phase 4, after the MVP is validated).**
  - The same core and runtime run on a server, with WebSocket replacing the Tauri transport and WebRTC carrying audio.
  - The UI is served as a web app.
  - Managed models and sync come with a subscription.

  The rule that keeps this cheap starts now: the UI reaches the runtime only through the typed protocol, and the core and runtime never import Tauri.

## 10. Decisions this design depends on

Each one has a row with its reason in [DECISIONS.md](../DECISIONS.md):

- Tauri
- The core / runtime / app split
- A cascaded voice pipeline
- Native audio with echo cancellation
- A custom renderer with Excalidraw-compatible export
- Preact
- The provider set
- Claude Code CLI and pi as harnesses
- Local-first files plus a derived index
- The memory policy
- Append-only LLM history
- Generated protocol types

## 11. Risks and spikes (Phase 0, first week)

Each spike has a pass bar. A failing spike changes the design before more code is built on it.

| Spike | Question | Pass bar | If it fails |
|---|---|---|---|
| **S1 Echo cancellation** | Can an open mic on laptop speakers barge in without BoardAI hearing itself? | Fewer than 1 false barge-in per 10 minutes of BoardAI speaking | Push-to-talk by default on speakers, open mic with headphones |
| **S2 Local voice** | Does Kokoro sound human enough, run fast enough, and give word timings? | Real-time factor ≤ 0.3 on M1; timings within 50 ms; no GPL code linked (English pronunciation through a lexicon, not espeak-ng) | ElevenLabs as the recommended default; the system voice stays as the free fallback |
| **S3 Talker latency** | Time to first token and how well each model follows the narration format | p50 ≤ 450 ms and ≥ 98% well-formed tags | Change the recommended default model (a product decision, recorded in DECISIONS) |
| **S4 Whisper partials** | Partial transcripts on Metal with `base.en` | A partial every ≤ 500 ms; final ≤ 150 ms after end of turn | A smaller model for partials, or a streaming model (Parakeet) behind the same interface |
| **S5 Narration across models** | Do Claude, an OpenRouter model and a local Ollama model all produce usable drawings? | Each one draws the demo diagram end to end | Mark the model as unsupported for drawing in the provider list |

**S3 and S5 with OpenRouter** (2026-10-16, funded account, strict Cerebras route (D-152) and context folded into user turns (D-160); `just converse --first-after 1500` on the four standard scripts in `crates/runtime/examples/converse/scripts`, two runs each per model, 12 answers per model):

| Model | First token (S3: p50 ≤ 450 ms) | Tags well-formed (S3: ≥ 98 %) | Draws the demo end to end (S5) |
|---|---|---|---|
| gpt-oss-120b, Cerebras, effort `low` | **418 ms p50**, 467 p90 (363–692); 399 ms p50 after the session warm-up (`just live`). Passes | **152 of 152 (100 %)**. Passes | Yes: 11 of 12 answers drew, first stroke p50 1.9 s (TCP within 0.45 s). Weak spot: the detour "Wait, why Redis first?" spoke 103 words and drew nothing in one run, and drew first at 6.8 s in the other (T-022 narration) |
| Qwen3.8 27B, Cerebras, thinking off | 488 ms p50, 947 p90 (423–3,368). Misses | 190 of 195 (97.4 %). Misses | Partly: 9 of 12 drew, but first stroke p50 11 s after 23–259 spoken words; three answers drew nothing |

gpt-oss-120b on Cerebras passes S3 and is the OpenRouter talker preset; Qwen3.8 27B misses both S3 bars as a talker (it talks first and draws late) and stays the scribe preset, where latency doesn't matter. Earlier runs on the unfunded account were cut short by routing to slower providers and then refusals (402), and every Qwen request had been refused by Cerebras's chat template until D-160.

**Steel thread first.** In week one, one thin path goes through every seam: mic → voice detection → Whisper → core → Claude → parser → text-to-speech → playback clock → op → stroke on screen. Fakes stand in wherever a real part isn't ready. Polish comes after that path works.
