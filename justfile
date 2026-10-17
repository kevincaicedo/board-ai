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
# ANTHROPIC_API_KEY is set, OpenRouter's presets when OPEN_ROUTER_KEY is set, and Claude Code /
# pi with BOARDAI_LIVE_CLAUDE=1 / BOARDAI_LIVE_PI=1 (they use your account). Keys come from the
# gitignored `.env` (scripts/with-dev-keys.sh).
live *filter:
    BOARDAI_LIVE=1 scripts/with-dev-keys.sh cargo test -p boardai-runtime --test live --test live_documents -- --ignored --nocapture --test-threads=1 {{filter}}

# The ElevenLabs live check, never part of `check`: three short clauses over the realtime
# WebSocket (the first on a cold connection) and one over HTTP, about 150 characters billed.
# Prints time to first audio, the alignment against the audio and the characters sent, and
# records the server's message shapes (audio redacted) in target/elevenlabs-live. Needs
# ELEVENLABS_API_KEY in the environment; it is never printed.
live-elevenlabs:
    BOARDAI_LIVE=1 cargo test -q -p boardai-runtime --lib elevenlabs_realtime_live -- --ignored --nocapture

# Model files (Whisper, Silero, Kokoro, lexicon) in the app's data folder: status, or download
# and verify the named ones (`all` for every one). `BOARDAI_MODELS_DIR` overrides the folder.
models *names:
    cargo run -q -p boardai-runtime --example models -- {{names}}

# Plays two clauses through VoiceProcessingIO with the system voice and prints the reported
# playback positions. Options: --text "…", --voice <id>, --list-voices, --input builtin|<name>,
# --output <name>, --list-inputs (the microphones by name, and whether this process may record;
# never prompts), --list-devices (every device and the defaults), --watch <seconds> (open the
# unit silently and print device events while you unplug, plug in or switch devices).
# Positional arguments keep quoted device names whole (`just audio-probe --input "Shure MV7+"`).
[positional-arguments]
audio-probe *args:
    cargo run -q -p boardai-runtime --example audio_probe -- "$@"

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

# The voice steel thread end to end, no human (LLD §7): macOS `say` speaks the user's lines into
# a file mic, and the VAD, Whisper, the core, the talker, the system voice, the playback clock
# and the ops run for real (the voice plays silently unless `--output speakers`). Writes
# target/e2e/{events,trace}.jsonl and prints the checks and HLD §8 latencies. Needs Ollama with
# gemma4:latest (or `--provider openrouter` and OPEN_ROUTER_KEY in `.env`) and
# `just models silero-vad whisper-base.en`. The vocabulary stands in for the opened repo's
# symbols, which the app gives Whisper. See `just e2e --help`.
e2e *args:
    scripts/with-dev-keys.sh cargo run -q --locked -p boardai-runtime --example e2e -- --vocabulary "Redis,Postgres" {{args}}

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
# Defaults to Ollama with gemma4:latest; `--provider openrouter` uses the presets with
# OPEN_ROUTER_KEY from `.env`. The standard scripts are in crates/runtime/examples/converse/scripts
# (`--script crates/runtime/examples/converse/scripts/barge_in.txt`); see `just converse --help`.
converse *ARGS:
    scripts/with-dev-keys.sh cargo run --quiet --locked -p boardai-runtime --example converse -- {{ARGS}}
