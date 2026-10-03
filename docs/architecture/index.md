---
title: "Architecture"
---

## Where code runs

| Where | What happens there |
| --- | --- |
| The browser | Editing, typesetting, rendering, offline copies |
| The server | Identity, authority, durability, live relay, review decisions |
| The author's machine | Quarto, Typst to HTML, Zotero |

Typesetting runs in WebAssembly modules in the browser. The deployment never compiles documents or stores compilers. LaTeX engines and TeX Live packages are fetched from an HTTPS mirror (configurable). Authority is decided server-side on every request, never inferred from the browser.

When backups are enabled, see [local backups](../backups.html).

## Records and caches

Records describe what somebody did at a moment and cannot be edited afterwards. Caches describe where something is now and are recomputed on every document change. A comment that loses its place reports that rather than being re-pointed.

## Trust model

1. The operator sees every draft, comment, identity and presence event in clear (no end-to-end encryption).
2. The document owner holds every authority.
3. A link holder holds exactly the role their link names; links expire.
4. Documents are hostile; bytes uploaded run their own scripts inside the reader.

Published documents are served from a second configured origin (separate host required). Documents may run their own code but not fetch code from other hosts. Messages between reader and document are origin-checked at both ends.

## Related pages

[Document and storage](document.html) | [Comments and track changes](review.html) | [Rendering and live sync](rendering.html) | [Companion and agents](companion.html) | [Building from source](building.html)
