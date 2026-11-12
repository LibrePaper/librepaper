#!/usr/bin/env bash
# Regenerates sample.docx, sample.odt and sample.html from sample.md.
# Run this after editing sample.md. The outputs are committed, so nobody
# needs pandoc to capture clipboards.
set -euo pipefail
cd "$(dirname "$0")"

title="Tidewater grant application"

pandoc -f markdown -t docx -o sample.docx sample.md
pandoc -f markdown -t odt -o sample.odt sample.md
pandoc -s --metadata title="$title" -f markdown -t html5 -o sample.html sample.md

echo "wrote sample.docx sample.odt sample.html in $(pwd)"
