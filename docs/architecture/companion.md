---
title: "Companion and agents"
---

## Companion app

`librepaper` runs a loopback service executing tools on the author's machine.

The companion handles two cases the browser cannot: Quarto and Calepin (both run the document's code). It reaches Zotero read-only.

Versioned protocol over loopback HTTP with bounded limits on body, upload, file count, PDF, log size and job deadline. Health endpoint identifies the service and negotiates version (nothing else). Requests with mismatched `Host` header are answered before parsing (DNS rebinding protection).

A browser pairs with the service per origin and project. Pairings live in the user's state directory keyed by origin and project. Only the token hash is written to disk; tokens expire.

An agent edits via MCP to the server (MCP owns transport, the room owns effects and durability). A caller supplies source tree identity and immutable byte ranges; the validator produces a new source. Patches whose ranges don't match are refused; validated batches apply in one commit.

Agent work arrives as a [tracked change](review.html#track-changes) branch and is reviewed and decided like human suggestions. Nothing enters the document without a decision. An agent's authority is its link; the caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or transcript store.

An agent reading a document reads untrusted text. The companion executes code from anyone with editor access; on shared documents, that means running collaborators' code on your machine.
