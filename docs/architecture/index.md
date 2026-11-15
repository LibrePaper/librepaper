---
title: "Architecture"
---

## Where code runs

| Where | What happens there |
| --- | --- |
| The browser | Editing, typesetting, rendering, offline copies |
| The server | Identity, authority, durability, live relay, review decisions |
| The author's machine | Local builds (Quarto, Calepin, latexmk), Zotero, local agents |

Typesetting runs in WebAssembly modules in the browser. The deployment never compiles documents or stores compilers. LaTeX engines and TeX Live packages come from a configurable HTTPS mirror. The server decides authority on every request, never from the browser.

## Records and caches

Records describe what somebody did at a moment and cannot be edited afterwards. Caches describe where something is now and are recomputed on every document change. A comment that loses its place reports that rather than being re-pointed.

## Trust model

1. The operator sees every draft, comment, identity and presence event in clear. Nothing is end-to-end encrypted.
2. The document owner holds every authority.
3. A link holder holds exactly the role their link names. Links expire.
4. Documents are hostile. Uploaded bytes run their own scripts inside the reader.

Published documents are served in a sandboxed frame from a second configured origin, which must be a separate host. That origin limits access to application credentials and authority. It does not make document content confidential from readers, and it does not block external resources. The content security policy allows HTTPS scripts and other network requests, so document code can contact third parties. Messages between reader and document are origin-checked at both ends.
