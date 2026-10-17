# BoardAI

**A whiteboard that talks back, and remembers.**

BoardAI is a voice-first desktop app for understanding software. You ask out loud, and it explains like a senior engineer at a whiteboard: it speaks while hand-drawn diagrams sketch themselves in sync with its voice. Interrupt it at any moment ("wait, why Redis?"). It stops mid-sentence and mid-stroke, answers in a detour, then picks up exactly where it left off.

It reads your real code through native tools and coding harnesses (Claude Code, pi), and it keeps notes, tasks and memories as plain files on your machine.

> **Status:** MVP built (October 2026): voice loop with barge-in, live drawing in sync with speech, detours and resume, code grounding, memory and library. It runs end to end on local models. See [PLAN](docs/PLAN.md) for what is left before the exit criteria.

## Principles

- **Voice first.** The screen is the whiteboard.
- **Open source, bring your own keys.**
  - LLM: Claude, OpenRouter or a local model through Ollama.
  - Speech: local Whisper and a local voice by default, with ElevenLabs as an option.
- **Local-first.** Your notes, sessions and memory live in a folder you own.

## Docs

| | |
|---|---|
| [Product](docs/PRODUCT.md) | What we're building and why |
| [High-level design](docs/architecture/HLD.md) · [diagram](docs/architecture/hld.excalidraw) | The parts and how they fit |
| [Low-level design](docs/architecture/LLD.md) · [diagrams](docs/architecture/lld.excalidraw) | Types, algorithms, limits, invariants |
| [Decisions](docs/DECISIONS.md) | Why things are the way they are |
| [Plan](docs/PLAN.md) | Where the work is |
| [AGENTS.md](AGENTS.md) | How we work: rules for people and agents |

The `.excalidraw` files open at [excalidraw.com](https://excalidraw.com) (menu → Open).

## Run it

You need macOS 14+ on Apple silicon, Rust (the version is pinned in `rust-toolchain.toml`), Node 24+, [`just`](https://github.com/casey/just) and [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny). For fully local use, [Ollama](https://ollama.com) with a model such as `gemma4:latest`.

```bash
npm --prefix ui install
just models all    # download and verify Whisper, Silero VAD and Kokoro (about 2 GB, once)
just dev           # run the app; the first launch shows the setup screen
```

The setup screen asks for your talker (Claude with your Anthropic key, OpenRouter, or a local Ollama model), checks the local models, and asks for microphone permission. Keys go to the macOS Keychain, never to a file. Reopen it any time with ⌘, or by saying "open settings".

Your settings live in `~/Library/Application Support/ai.boardai.desktop/settings.toml`, and your notes, sessions and memories in `~/BoardAI` (plain Markdown and `.excalidraw` files).

> If your default input device is a virtual one, choose the built-in microphone on the setup screen (or set `[audio] input = "built_in"`). `just audio-probe --list-inputs` lists the inputs by name.

## Develop

```bash
just check         # everything CI runs: lint, types, tests, builds, cargo deny, protocol, file lengths
just converse      # text-only dev loop against the real talker (Ollama by default); prints ops and latency
just e2e           # the voice pipeline end to end with no human: a `say` file mic, Whisper, the core, the talker, the voice
just live          # real-provider smoke tests (Ollama; Anthropic and OpenRouter when their keys are set)
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
