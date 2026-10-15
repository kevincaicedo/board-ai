set shell := ["bash", "-euo", "pipefail", "-c"]

# Everything CI runs. Green before every commit.
check: ui rust protocol files

# UI lint + format check, typecheck, unit tests, then the production build (the app crate embeds it).
ui:
    npm --prefix ui run check
    npm --prefix ui run build

rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    cargo deny --locked check

# The UI's protocol types, generated from crates/core, are current.
protocol:
    scripts/check-protocol.sh

files:
    scripts/check-file-length.sh

# Real-provider smoke tests, never part of `check` or CI: Ollama (local), Anthropic when
# ANTHROPIC_API_KEY is set, and Claude Code / pi with BOARDAI_LIVE_CLAUDE=1 / BOARDAI_LIVE_PI=1
# (they use your account).
live:
    BOARDAI_LIVE=1 cargo test -p boardai-runtime --test live -- --ignored --nocapture --test-threads=1

# Model files (Whisper, Silero, Kokoro, lexicon) in the app's data folder: status, or download
# and verify the named ones (`all` for every one). `BOARDAI_MODELS_DIR` overrides the folder.
models *names:
    cargo run -q -p boardai-runtime --example models -- {{names}}

# Plays two clauses through VoiceProcessingIO with the system voice and prints the reported
# playback positions. Options: --text "…", --voice <id>, --list-voices, --input builtin.
audio-probe *args:
    cargo run -q -p boardai-runtime --example audio_probe -- {{args}}

# Spike S4 (HLD §11): Whisper partial cadence and final latency on `say` speech.
spike-s4:
    cargo test -q -p boardai-runtime --test spikes -- --ignored --nocapture --test-threads=1 spike_s4

# Spike S2 (HLD §11): Kokoro real-time factor and timing accuracy; writes sample WAVs.
spike-s2:
    cargo test -q -p boardai-runtime --test spikes -- --ignored --nocapture --test-threads=1 spike_s2

# Spike S1 (HLD §11): the system voice plays through VoiceProcessingIO while the VAD listens on
# its mic; counts false voice starts. Needs speakers and microphone permission for the terminal.
# Pass `--minutes N` (default 10) and `--input builtin` to use the Mac's own microphone.
spike-s1 *args:
    cargo run -q -p boardai-runtime --example spike_s1 -- {{args}}

# Run the desktop app with hot reload.
dev:
    cd crates/app && ../../ui/node_modules/.bin/tauri dev

# Regenerate the UI's protocol types after changing crates/core/src/protocol.rs.
protocol-write:
    scripts/check-protocol.sh --write

fmt:
    cargo fmt --all
    npm --prefix ui run format

# Text-only dev loop: type to BoardAI, watch the narration, ops and latency marks (no audio).
# Defaults to Ollama with gemma4:latest; see `just converse --help` for providers and scripts.
converse *ARGS:
    cargo run --quiet --locked -p boardai-runtime --example converse -- {{ARGS}}
