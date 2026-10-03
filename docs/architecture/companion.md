---
title: "Companion and agents"
---

## Companion app

`librepaper` runs a loopback service executing tools on the author's machine. Discovery resolves local tools to absolute paths.

The companion handles two cases the browser cannot: Quarto (code execution) and Calepin (self-contained HTML from Typst with embedded images). It reaches Zotero read-only. Everything else compiles in the browser. Typst produces PDF there; LaTeX and Biber are WebAssembly only.

Versioned protocol over loopback HTTP with bounded limits on body, upload, file count, PDF, log size and job deadline. Health endpoint identifies the service and negotiates version (nothing else). Requests with mismatched `Host` header are answered before parsing (DNS rebinding protection).

A browser pairs with the service per origin and project. Pairings live in the user's state directory keyed by origin and project. Only the token hash is written to disk; tokens expire.

A browser cannot enumerate applications; the client makes one bounded probe and remembers the outcome.

Presets are owned by the companion. Only opaque id and metadata cross to the browser; paths, arguments and environment stay on the machine. Presets may not set loader or interpreter environment variables.

An agent edits via MCP to the server (not loopback). MCP owns transport; the room owns effects and durability. The patch language is independent of MCP. A caller supplies source tree identity and immutable byte ranges; the validator produces a new source without guessing anchors. Patches whose ranges don't match are refused. Validated batches apply across all text files in one commit. The operation persists before changes; the receipt commits only after durability.

Agent work arrives as a [tracked change](review.html#track-changes) branch and is reviewed and decided like human suggestions. Nothing enters the document without a decision. An agent's authority is its link; the caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or transcript store.

A document is written by editor-access holders; an agent reading one reads untrusted text. Agents on shared documents should be treated as acting on everyone's instructions. The companion executes code from anyone with editor access; running it against shared documents means running collaborators' code on your machine.
