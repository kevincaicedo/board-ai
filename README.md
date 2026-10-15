# BoardAI

**A whiteboard that talks back, and remembers.**

BoardAI is a voice-first desktop app for understanding software. You ask out loud, and it explains like a senior engineer at a whiteboard: it speaks while hand-drawn diagrams sketch themselves in sync with its voice. Interrupt it at any moment ("wait, why Redis?"). It stops mid-sentence and mid-stroke, answers in a detour, then picks up exactly where it left off.

It reads your real code through native tools and coding harnesses (Claude Code, pi), and it keeps notes, tasks and memories as plain files on your machine.

> **Status:** pre-MVP. The architecture is in review, and Phase 0 ("the magic trick") is next.

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

## Develop

You need macOS 14+ on Apple silicon, Rust (the version is pinned in `rust-toolchain.toml`), Node 24+, [`just`](https://github.com/casey/just) and [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny).

```bash
npm --prefix ui install
just dev      # run the app with hot reload
just check    # everything CI runs
```

## License

[Apache-2.0](LICENSE)
