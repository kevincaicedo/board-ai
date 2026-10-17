# BoardAI

**The board for understanding.**

BoardAI is a voice-first desktop app for understanding, planning and designing software with an AI. Drop in a codebase, a PDF or a Markdown document and ask out loud. It explains while hand-drawn diagrams sketch themselves in sync with its voice, and you can interrupt, draw or say "note that" at any moment. Your coding harness (Claude Code, pi) researches and writes the code; BoardAI is where you understand it and decide what to do next.

Open source (Apache-2.0), bring your own keys, local-first. The repository is [github.com/kevincaicedo/board-ai](https://github.com/kevincaicedo/board-ai).

> **Status:** MVP built (October 2026), now being validated in real study sessions. It runs end to end on local models.

## What works today

- **The voice loop with barge-in.** Open mic or push-to-talk, with captions. Interrupt at any moment ("wait, why Redis?"): BoardAI stops mid-sentence and mid-stroke, answers in a detour, then picks up exactly where it left off. A "mm-hm" doesn't stop it.
- **Live drawing.** Hand-drawn diagrams drawn stroke by stroke in sync with speech, from a quick sketch to a staged detailed diagram (nested containers, a legend, records with fields, numbered steps) revealed a part at a time. "More detail on X" drills into a part, and "back out" returns.
- **You draw.** Pick up the pen, sketch, name the parts by voice, say "done", and BoardAI answers about your drawing.
- **Sources.** Share code (a repo, a crate, a module or a file) or a document (PDF, Markdown or text) by dropping it on the window, from the Sources tray, or by voice, for this session or kept in a project.
- **Research briefs from Claude Code or pi.** Questions about code go to your coding harness, confined to the source's repository. BoardAI draws the brief's map and explains it, and checks every `file:line` the brief cites before its chip shows.
- **Document digests.** PDF and Markdown digests that keep tables and figures. Page citations are checked and open on the Doc card, and a figure can be redrawn on the board.
- **Memory, notes, tasks and replays.** Notes and tasks by voice, a digest at the end of each session, recall ("where were we on the API stuff?"), "forget that", and a library of past sessions you can replay.
- **The settings page.** Providers and models: Claude, OpenRouter (any listed model, with measured presets such as gpt-oss-120b, DeepSeek V4.1 Flash and Gemini 3.8 Flash), or a local model through Ollama. Voices: Kokoro on your Mac, ElevenLabs, or the macOS system voice. Also the microphone, keys (kept in the macOS Keychain), coding harnesses and code folders.

Also built: code cards that highlight in sync with speech, trace mode, the Compare surface, pointing ("what's this?") and fresh pages ("clear the board").

## Roadmap

The MVP's scope stays as it is while it's validated. After it, new functionality ships in phases, each with an exit test ([PRODUCT §11](docs/PRODUCT.md#11-roadmap)):

1. **Validate the MVP:** real study sessions, Claude measured, polish from use.
2. **Co-design:** brainstorming, wireframes and mock-ups, architecture diagrams, and designs handed to Claude Code.
3. **Project planning:** the Board and Roadmap views, and tasks sent to the harness.
4. **The BoardAI MCP server and repo URLs:** the harness pulls context from BoardAI, and BoardAI downloads a repo from its URL.
5. **The personal brain:** the knowledge graph view, expertise, learning path and preferences.

Later: a hosted version, sync and multiplayer.

## Docs

| | |
|---|---|
| [Product](docs/PRODUCT.md) | What we're building and why: vision, use cases, MVP scope, roadmap |
| [High-level design](docs/architecture/HLD.md) · [diagram](docs/architecture/hld.excalidraw) | The parts and how they fit |
| [Low-level design](docs/architecture/LLD.md) · [diagrams](docs/architecture/lld.excalidraw) | Types, algorithms, limits, invariants |
| [AGENTS.md](AGENTS.md) | How we work: rules for people and agents |

The `.excalidraw` files open at [excalidraw.com](https://excalidraw.com) (menu → Open). Decisions are cited by id (`D-123`); the decision log and the plan are kept by the maintainer outside this repository.

## Run it

You need macOS 14+ on Apple silicon, Rust (the version is pinned in `rust-toolchain.toml`), Node 24+, [`just`](https://github.com/casey/just) and [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny). For fully local use, [Ollama](https://ollama.com) with a model such as `gemma4:latest`. To understand code, install a coding harness: [Claude Code](https://claude.com/claude-code) or pi.

```bash
npm --prefix ui install
just models all    # download and verify Whisper, Silero VAD and Kokoro (about 2 GB, once)
just dev           # run the app; the first launch shows the setup screen
```

The setup screen asks for your talker (Claude with your Anthropic key, OpenRouter, or a local Ollama model), checks the local models, and asks for microphone permission. Keys go to the macOS Keychain, never to a file. Change anything later on the settings page: press ⌘, or say "open settings".

Your settings live in `~/Library/Application Support/ai.boardai.desktop/settings.toml`, and your notes, sessions and memories in `~/BoardAI` (plain Markdown and `.excalidraw` files).

> If your default input device is a virtual one, choose the built-in microphone in settings (or set `[audio] input = "built_in"`). `just audio-probe --list-inputs` lists the inputs by name.

## Develop

```bash
just check         # everything CI runs: lint, types, tests, builds, cargo deny, protocol, file lengths
just converse      # text-only dev loop against the real talker (Ollama by default); prints ops and latency
just e2e           # the voice pipeline end to end with no human: a `say` file mic, Whisper, the core, the talker, the voice
just live          # real-provider smoke tests (Ollama; Anthropic and OpenRouter when their keys are set; Claude Code and pi with BOARDAI_LIVE_CLAUDE=1 / BOARDAI_LIVE_PI=1)
just audio-probe   # play through VoiceProcessingIO and print playback positions
just spike-s1      # echo-cancellation spike: run in a quiet room on laptop speakers
```

**Development keys.** Put the keys the dev tools use in a `.env` file at the repo root (it is gitignored), one `NAME=value` per line: `OPEN_ROUTER_KEY`, `ELEVENLABS_API_KEY`, `ANTHROPIC_API_KEY`. `just converse`, `just e2e` and `just live` load it into their own environment only (`scripts/with-dev-keys.sh`; a git worktree uses the main checkout's). Never paste a key on a command line. The app never reads keys from the environment, in debug or release builds: paste them on the setup screen, which keeps them in the Keychain.

```bash
just converse --provider openrouter                       # gpt-oss-120b on Cerebras (the preset)
just converse --provider openrouter --model qwen/qwen3.8-27b \
  --script crates/runtime/examples/converse/scripts/barge_in.txt
just e2e --provider openrouter                            # voice to voice with an OpenRouter talker
```

Open the UI alone with `npm --prefix ui run dev`: outside the app it plays the design's demo from a replay file (`?replay=/fixtures/demo.events.jsonl&t=<ms>`).

## License

[Apache-2.0](LICENSE)
