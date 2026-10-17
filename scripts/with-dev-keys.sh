#!/usr/bin/env bash
# Runs a dev tool (`converse`, `e2e`, `live`) with the development keys from the gitignored
# `.env` in its environment, and only in its environment: OPEN_ROUTER_KEY, ELEVENLABS_API_KEY,
# ANTHROPIC_API_KEY. In a git worktree without its own `.env`, the main checkout's is used. The
# keys are never printed, and the app never reads keys from the environment (D-157): it uses
# the Keychain.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
env_file="$root/.env"
if [[ ! -f "$env_file" ]]; then
  common="$(git -C "$root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
  if [[ -n "$common" && -f "$(dirname "$common")/.env" ]]; then
    env_file="$(dirname "$common")/.env"
  fi
fi
if [[ -f "$env_file" ]]; then
  set -a
  # shellcheck disable=SC1090
  . "$env_file"
  set +a
fi
exec "$@"
