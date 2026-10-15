set shell := ["bash", "-euo", "pipefail", "-c"]

# Everything CI runs. Green before every commit.
check: ui rust files

# UI lint + format check, typecheck, unit tests, then the production build (the app crate embeds it).
ui:
    npm --prefix ui run check
    npm --prefix ui run build

rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    cargo deny --locked check

files:
    scripts/check-file-length.sh

# Run the desktop app with hot reload.
dev:
    cd crates/app && ../../ui/node_modules/.bin/tauri dev

fmt:
    cargo fmt --all
    npm --prefix ui run format
