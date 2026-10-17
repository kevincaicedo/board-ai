#!/bin/bash
# A stand-in for `claude` and `pi`: records its arguments and stdin, replays a fixture, then
# behaves as FAKE_MODE says. `{REPO}` in the fixture becomes the working directory. FAKE_DELAY
# (seconds) holds the answer back, as a harness still reading would.
printf '%s\n' "$@" > "$FAKE_FOLDER/arguments"
echo $$ > "$FAKE_FOLDER/process-id"
if [ -n "$FAKE_DELAY" ]; then sleep "$FAKE_DELAY"; fi
IFS= read -r first_line
printf '%s\n' "$first_line" > "$FAKE_FOLDER/stdin"
replay() { sed "s|{REPO}|$(pwd -P)|g" "$FAKE_FIXTURE"; }
case "$FAKE_MODE" in
  replay) replay; cat > "$FAKE_FOLDER/rest" ;;
  replay-exit) replay; echo "fake harness: done early" >&2 ;;
  hang) trap 'echo interrupted >&2; exit 130' INT; replay; while true; do sleep 0.05; done ;;
  ignore-int) trap '' INT; replay; while true; do sleep 0.05; done ;;
  crash) echo "boom: something broke" >&2; exit 3 ;;
  # Exits before its last stderr line is written: a child keeps stderr (not stdout) open.
  crash-late) ( exec >/dev/null; sleep 0.2; echo "boom: said late" >&2 ) & exit 3 ;;
esac
