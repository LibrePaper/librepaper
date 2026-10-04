---
title: "Companion and agents"
---

## Companion app

`librepaper` runs a loopback service executing tools on the author's machine.

The companion handles two cases the browser cannot: Quarto and Calepin (both run the document's code). It reaches Zotero read-only.

Versioned protocol over loopback HTTP with bounded limits on body, upload, file count, PDF, log size and job deadline. Health endpoint identifies the service and negotiates version (nothing else). Requests with mismatched `Host` header are answered before parsing (DNS rebinding protection).

A browser pairs with the service per origin and project. Pairings live in the user's state directory keyed by origin and project. Only the token hash is written to disk; tokens expire.

On supported desktops, starting the companion shows a tray icon. Choose **Settings** from its menu to open the Companion section in the main LibrePaper app, defaulting to `https://app.librepaper.org/`; setting `LIBREPAPER_SERVER` when the companion starts selects a self-hosted app. A per-instance control token travels in the URL fragment, is removed before the app mounts, and stays in that tab's session storage. This credential is separate from document pairing, and the API accepts it only from the configured app origin. Settings show pending approvals, available tools, connected sites, authorized folders, running work and local configuration. Revoking a site also removes its scoped folder grants and related local work.

Closing the app page leaves the companion running. Use **Quit companion** in Settings or `librepaper stop` to stop it. Start-at-login remains optional. Tray support depends on the desktop environment; where it is unavailable, use the command line. Folder selection opens the operating system's native chooser, and folder names and contents remain local. On a headless machine, pairing can still be approved with `librepaper local approve <code>` from the terminal.

An agent edits via MCP to the server (MCP owns transport, the room owns effects and durability). A caller supplies source tree identity and immutable byte ranges; the validator produces a new source. Patches whose ranges don't match are refused; validated batches apply in one commit.

Agent work arrives as a [tracked change](review.html#track-changes) branch and is reviewed and decided like human suggestions. Nothing enters the document without a decision. An agent's authority is its link; the caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or transcript store.

An agent reading a document reads untrusted text. The companion executes code from anyone with editor access; on shared documents, that means running collaborators' code on your machine.
