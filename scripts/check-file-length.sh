#!/usr/bin/env bash
# Every source file stays at or under MAX_LINES; past that it becomes a module folder (AGENTS.md).
# The check asserts its own scope: finding no files is a failure, not a pass.
# Written for macOS's bash 3.2 (no mapfile).
set -euo pipefail

readonly MAX_LINES=800
cd "$(dirname "$0")/.."

scanned=0
status=0
while IFS= read -r file; do
  scanned=$((scanned + 1))
  lines=$(wc -l < "$file" | tr -d ' ')
  if (( lines > MAX_LINES )); then
    echo "check-file-length: FAIL: $file has $lines lines (max $MAX_LINES)" >&2
    status=1
  fi
done < <(find crates ui/src -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' \) \
  -not -path '*/generated/*' -not -path '*/target/*' -not -path '*/node_modules/*' | sort)

if (( scanned == 0 )); then
  echo "check-file-length: FAIL: no source files found under crates/ and ui/src" >&2
  exit 1
fi
if (( status == 0 )); then
  echo "check-file-length: OK: $scanned files scanned, all <= $MAX_LINES lines"
fi
exit "$status"
