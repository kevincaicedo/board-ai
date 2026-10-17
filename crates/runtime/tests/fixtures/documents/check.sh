#!/usr/bin/env bash
# For humans, not CI: runs BoardAI's PDFKit script (`text` mode) on each fixture PDF and prints
# its page count and each page's first lines with their font size, boldness and box, in PDFKit's
# order (the runtime reads them in its own, `documents::order`, D-293).
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
        box = " ".join("%6.1f" % value for value in line["box"])
        print("    %5.1f %s [%s] %s" % (line["size"], "B" if line["bold"] else " ", box, line["text"].strip()[:60]))
'
done
