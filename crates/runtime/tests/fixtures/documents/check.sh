#!/usr/bin/env bash
# For humans, not CI: runs BoardAI's PDFKit script (`text` mode) on each fixture PDF and prints
# its page count and each page's first lines with their font size and boldness.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
script="$here/../../../src/documents/pdfkit.js"

for pdf in "$here"/*.pdf; do
  echo "== $(basename "$pdf")"
  /usr/bin/osascript -l JavaScript "$script" text "$pdf" 50 2>/dev/null | python3 -I -c '
import json, sys
document = json.load(sys.stdin)
if "error" in document:
    print("  error:", document["error"])
    sys.exit()
print("  pages:", document["page_count"], "locked:", document["locked"])
for number, page in enumerate(document["pages"], start=1):
    lines = page["lines"]
    print("  p.%d: %d lines" % (number, len(lines)))
    for line in lines[:4]:
        print("    %5.1f %s %s" % (line["size"], "B" if line["bold"] else " ", line["text"][:70]))
'
done
