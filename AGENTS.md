# BoardAI

BoardAI is a voice-first desktop app for understanding software. You talk, it explains while drawing hand-drawn diagrams in sync with its voice, you interrupt, and it resumes exactly where it stopped. It is open source (Apache-2.0), with bring-your-own-keys providers.

Stack:

- Tauri 2, macOS first
- A pure Rust **core**, a Rust **runtime** for all I/O, and a thin **app** shell
- A Preact and TypeScript **ui**

We move fast, without taking on debt that is expensive to remove later. Process stays lean, and code quality does not bend.

## Authority

When documents conflict, the higher one wins. A document that the code contradicts is fixed in the same change.

1. [docs/PRODUCT.md](docs/PRODUCT.md): what we build and why.
2. The [design canvas](https://claude.ai/artifact/XY3ZrPFmFDA9gNw9Y6WuVJ): how it looks and moves. It is the source of truth for every screen. Agents with claude.ai access read it with the Artifact tool. Everyone else uses the tokens exported into `ui/src/styles/`.
3. [docs/architecture/HLD.md](docs/architecture/HLD.md) and [LLD.md](docs/architecture/LLD.md): how it works.
4. [docs/DECISIONS.md](docs/DECISIONS.md): why it works that way.
5. [docs/PLAN.md](docs/PLAN.md): where the work is.

Before changing an area, read the HLD and the LLD sections it touches. Before UI work, read the canvas frame you're implementing.

## Process: three records, nothing else

| Record | Updated when |
|---|---|
| `docs/DECISIONS.md` row | **Before** the code, whenever a seam, dependency, protocol, storage format or product scope changes |
| `docs/PLAN.md` | In the same change that starts or finishes a task |
| HLD / LLD | In the same change that makes them wrong |

There are no tickets, journals or status reports. Commit messages and PR descriptions carry the rest.

**The work loop:** read → write the decision row if one is needed → build a thin slice with tests → `just check` → commit → update PLAN.

**Steel thread first.** Build one thin path through every seam before polishing any of them.

**Spikes have pass bars** (HLD §11). A failed spike changes the design before more code is built on it.

**Done** means all of these:

- The behavior is reachable in the app, or through the conversation simulator for core-only work.
- Tests cover it, and `just check` is green.
- Any document it contradicts is updated.
- You've said what you did not run, and why.

**Ask the owner only for:**

- Product decisions: default models, pricing, license, scope.
- Accounts and keys.
- What only a human can do: OS permission prompts, signing, judging how a voice sounds.

Decide everything else, record it if it is a decision, and keep going.

## Laws

1. **The core decides, the runtime does, the UI shows.** The core has no I/O, async, clock, threads or randomness. The runtime makes no conversation decisions. The UI holds no state the core owns.
2. **Time and randomness are inputs.** This is what makes the conversation simulator and replay possible. Clippy bans the clock in core.
3. **One owner per fact.**
   - The orchestrator task alone owns `Core`.
   - Store is the only writer.
   - The vault is the source of truth, and the SQLite index is derived from it.
4. **Append-only.** The session event log and the LLM conversation are never edited. An interruption commits only what you heard (LLD §2.7).
5. **Everything is bounded.** Every queue, loop, retry, buffer, timeout and output has a named limit with a unit, an owner and a defined crossing behavior (LLD §5). A limit that exists only as a literal is a defect.
6. **Never claim what wasn't read or heard.** Code claims come from files the session actually read, with `file:line`. Committed speech is the heard prefix.
7. **The real-time audio thread never allocates, locks, logs or blocks.**
8. **The design canvas is the source of truth for the UI.** A UI change starts on the canvas, or lands there in the same change.
9. **Dependencies are few and deliberate.** A new crate or npm package needs a DECISIONS row first. Prefer a short function to a dependency.
10. **Private by default.** Secrets never reach disk or logs, your voice is never stored, and data leaves only for the providers you configured.

## Rust rules

Design goals, in order: **correctness, then latency and performance, then developer experience.**

- **Make invalid states unrepresentable.**
  - Use newtypes for ids, times and offsets (`SessionTime`, `RepoPath`, `HeardPrefix`).
  - Use enums for state machines and outcomes: three outcomes are an enum, never `Option` plus `bool`.
  - Use typestate where order matters.
- **Parse, don't validate, at trust boundaries.** Anything from a provider, the filesystem, a subprocess or the UI is checked once where it enters, into a type. Inner code takes the type and doesn't re-check.
- **Errors are values.**
  - Use typed error enums that keep their category (`Auth`, `RateLimited { retry_after }`, `Network`, `Refused`, `BadOutput`).
  - Never turn a failure into success, or a refusal into "no answer".
  - `unwrap` and `expect` are denied. A rare invariant-backed `expect` carries `#[allow(clippy::expect_used, reason = "…")]`.
- **Panics mean a broken invariant**, never bad input or a failed request. Use `debug_assert!` for internal invariants. Pair checks at boundaries: assert what you emit, validate what you receive.
- **Matches are exhaustive.** No wildcard arm on our own enums.
- **Size limits.** Functions have at most 70 lines (clippy `too_many_lines`). Files have at most 800 lines (`scripts/check-file-length.sh`); past that, a file becomes a module folder.
- **Control flow is simple and explicit.** Use `?`, `let … else` and early returns. No recursion in parsers; use explicit, bounded state machines.
- **Async rules:**
  - Never hold a lock, borrow or guard across `.await`.
  - Dropping a future must be a valid cancellation.
  - Every spawned task has an owner and a shutdown path.
  - Inference (Whisper, Kokoro, VAD) runs on dedicated threads, never on the orchestrator task.
- **No `unsafe`** outside `runtime::audio::coreaudio`. Every `unsafe` block there has a `// SAFETY:` comment stating the exact argument.
- **Naming:**
  - Spell words out. The allowed abbreviations are `id`, `ms`, `pcm`, `vad`, `stt`, `tts`, `llm`, `ui`, `io`, `ctx`, `config` and `index`.
  - Units and qualifiers go last: `latency_ms_p50`, `budget_ms`, `ring_capacity_frames`.
  - Keep `index`, `count` and `size` distinct. Use half-open ranges `[begin, end)`.
- **Comments say why:** a constraint, an invariant, or a rejected alternative. Never what the next line does. No commented-out code. A `TODO` must point to a PLAN task.
- **Formatting:** `cargo fmt` with edition 2024 and a 100-column limit.

## TypeScript and UI rules

- **Strict TypeScript.** No `any`, and no non-null assertions except the boot lookup of `#app`. Only erasable syntax (no `enum`, no `namespace`), so `node --test` runs the sources directly.
- **Two rendering paths.** Components render surfaces and views. The canvas and orb are imperative modules driven by `requestAnimationFrame`, writing SVG directly. Avoid per-frame allocation on those paths.
- **The UI only knows the protocol.** Use the generated types in `ui/src/protocol/generated/` and the `Transport` interface. Never call Tauri APIs from components.
- **Exact values come from the design system.** Colors, sizes and timings are tokens in `ui/src/styles/tokens.css`, copied from the canvas. Never use a raw hex in a component.
- **Biome** settles lint and format.

## Tests

- **A bug starts red.** Keep the smallest failing reproducer as the regression test.
- **Test at the layer that owns the behavior:**
  - Conversation behavior: a simulator scenario in `crates/core`.
  - Parsers: golden files plus the split-invariance property.
  - Adapters: recorded fixtures (SSE, stream-json, WAV).
  - UI logic: `node --test`.
- **Test the failure paths** as well as the happy path: refusal, timeout, cancellation, malformed provider output, a full queue.
- **A check that cannot fail is not a check.** A new gate or property comes with a planted violation that turns it red.
- **CI never uses the network or real keys.** `just live` runs real-provider smoke tests locally.

## Performance

- The budgets are in HLD §8. The runtime records tracing marks for each stage, and the dev overlay shows them.
- An optimization is a hypothesis until measured. State the before and after numbers, and how you measured, in the PR.
- Keep the common path direct. Don't add dynamic dispatch, clones or allocation to per-sample or per-frame code. Per-request seams may use trait objects.

## Commands

```bash
just check      # everything CI runs: UI lint, typecheck, tests and build; cargo fmt, clippy -D warnings, tests, cargo deny; file lengths
just dev        # run the desktop app with hot reload
just fmt        # format Rust and TypeScript
```

The first time, run `npm --prefix ui install`. CI runs `just check` on macOS.

## Commits and output

- **Commits:** one short imperative line per logical change, for example `add narration parser`. Reasoning, measurements and commands go in the PR description.
- **Implementation reports:** lead with what changed. List files, what ran, and what did **not** run.
- **Reviews:** findings first, ordered by severity, each with `file:line`.
- **Ownership:** Claude drives implementation and keeps PLAN current. The owner reviews architecture and product decisions.
