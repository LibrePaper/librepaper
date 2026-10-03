---
title: "Architecture"
---

## Where code runs

| Where | What happens there |
| --- | --- |
| The browser | Editing, typesetting, rendering, offline copies |
| The server | Identity, authority, durability, live relay, review decisions |
| The author's machine | Quarto, Calepin, Zotero, local agents |

Typesetting runs in WebAssembly modules in the browser. The deployment never compiles documents or stores compilers. LaTeX engines and TeX Live packages are fetched from an HTTPS mirror (configurable). Authority is decided server-side on every request, never inferred from the browser.

## Records and caches

Records describe what somebody did at a moment and cannot be edited afterwards. Caches describe where something is now and are recomputed on every document change. A comment that loses its place reports that rather than being re-pointed.

## Trust model

1. The operator sees every draft, comment, identity and presence event in clear (no end-to-end encryption).
2. The document owner holds every authority.
3. A link holder holds exactly the role their link names; links expire.
4. Documents are hostile; bytes uploaded run their own scripts inside the reader.

Published documents are served in a sandboxed frame from a second configured origin (a separate host is required). The separate origin limits access to application credentials and authority; it does not make document content confidential from readers, nor does it block all external resources. The deployed content security policy permits HTTPS scripts and other network requests, so document code can contact third parties. Messages between reader and document are origin-checked at both ends. See [the response policy](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/specs/SPEC-security.md#rendering-and-outbound-requests) for the implemented policy and proposed restrictions.
