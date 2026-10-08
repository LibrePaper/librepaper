---
title: "Companion and agents"
---

## Companion app

`librepaper` runs a loopback service executing tools on the author's machine.

The companion handles two cases the browser cannot: Quarto and Calepin, which both run the document's code. It reaches Zotero read-only.

The protocol is versioned and runs over loopback HTTP. It sets bounded limits on body, upload, file count, PDF, log size and job deadline. The health endpoint identifies the service and negotiates version. It does nothing else. Requests with a mismatched `Host` header are answered before parsing. This protects against DNS rebinding.

A browser pairs with the service per origin and project. Pairings live in the user's state directory keyed by origin and project. Only the token hash is written to disk. Tokens expire.

On supported desktops, starting the companion shows a tray icon. Choose **Settings** from its menu to open the Companion section in the main LibrePaper app. The default app is `https://app.librepaper.org/`. Setting `LIBREPAPER_SERVER` when the companion starts selects a self-hosted app. The configured app pairs without a consent step. A JSON POST to `companion/api/session` from its exact Origin, loopback Host and peer returns an ordinary site pairing. That one pairing also carries machine management, so the app has a single connection. Pairings for other sites never reach the management routes. Settings show pending approvals, available tools, connected sites, authorized folders, running work and local configuration. Other sites pair through the public `pair/request` flow. Their consent request opens Settings in the configured app, which then connects on its own. The Settings link carries only the companion address, never a credential. The local `control-token.json` file is used only by the CLI `approve` command. Revoking a site also removes its scoped folder grants and related local work.

Closing the app page leaves the companion running. Use **Quit companion** in Settings or `librepaper stop` to stop it. Start-at-login remains optional. Tray support depends on the desktop environment. Where it is unavailable, use the command line. Folder selection opens the operating system's native chooser. Folder names and contents remain local. On a headless machine, pairing can still be approved with `librepaper local approve <code>` from the terminal.

An agent edits via MCP to the server. MCP owns transport, and the room owns effects and durability. A caller supplies source tree identity and immutable byte ranges. The validator produces a new source. Patches whose ranges don't match are refused. Validated batches apply in one commit.

Agent work arrives as a [tracked change](review.html#track-changes) branch and is reviewed and decided like human suggestions. Nothing enters the document without a decision. An agent's authority is its link. The caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or transcript store.

An agent reading a document reads untrusted text. The companion executes code from anyone with editor access. On shared documents, that means running collaborators' code on your machine.
