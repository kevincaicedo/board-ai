#!/usr/bin/env bash
# The UI's protocol types are generated from crates/core with ts-rs and committed. This exports
# them to a scratch folder and fails when the committed copy differs (LLD §3.7).
# `scripts/check-protocol.sh --write` refreshes the committed copy instead.
set -euo pipefail

cd "$(dirname "$0")/.."
readonly committed="ui/src/protocol/generated"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

TS_RS_EXPORT_DIR="$scratch" cargo test --quiet --locked -p boardai-core --lib export_bindings \
  >/dev/null

if [[ -z "$(ls -A "$scratch")" ]]; then
  echo "check-protocol: FAIL: ts-rs exported nothing" >&2
  exit 1
fi

if [[ "${1:-}" == "--write" ]]; then
  rm -rf "$committed"
  mkdir -p "$committed"
  cp "$scratch"/*.ts "$committed"/
  echo "check-protocol: wrote $(ls "$committed" | wc -l | tr -d ' ') files to $committed"
  exit 0
fi

if ! diff -r "$scratch" "$committed" >/dev/null; then
  diff -r "$scratch" "$committed" >&2 || true
  echo "check-protocol: FAIL: $committed is stale; run 'just protocol'" >&2
  exit 1
fi
echo "check-protocol: OK: $(ls "$committed" | wc -l | tr -d ' ') generated files are current"
