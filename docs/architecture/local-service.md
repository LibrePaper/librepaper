---
title: "Local service and agents"
---

## Local service

The desktop app runs a loopback service, the `librepaper` binary in local mode. It executes tools on the author's machine.

The local service handles two cases the browser cannot: Quarto and Calepin, which both run the document's code. It reaches Zotero read-only.

The protocol is versioned and runs over loopback HTTP. It sets bounded limits on body, upload, file count, PDF, log size and job deadline. The health endpoint identifies the service and negotiates version. It does nothing else. Requests with a mismatched `Host` header are answered before parsing. This protects against DNS rebinding.

The local service writes a local credential to `control-token.json` (mode 0600) in its state directory. The desktop app holds that credential. Browsers never call the local service directly.

- Every route except `GET /librepaper/local/health` requires `Authorization: Bearer <credential>`.
- The desktop app forwards each browser request through its `lp-local` scheme. It sends the page origin as `X-LibrePaper-Origin`.
- A request with a browser `Origin` or `Sec-Fetch-Site` header gets 403.

The desktop app chooses the server. Its default server is `https://app.librepaper.org/`. `LIBREPAPER_SERVER` selects a self-hosted server.

`librepaper local endpoint` starts the local service if it is not running. It prints one line of JSON with the address and credential. The desktop app runs it at startup and again after a failed request.

Users never start, stop or pair the local service. The desktop app runs it. Folder selection opens the operating system's native chooser. Folder names and contents remain local.

An agent edits via MCP to the server. MCP owns transport, and the room owns effects and durability. A caller supplies source tree identity and immutable byte ranges. The validator produces a new source. Patches whose ranges don't match are refused. Validated batches apply in one commit.

Agent work arrives as [tracked changes](review.html#track-changes) in the document, and editors accept or reject each one. An agent's authority is its link. The caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or transcript store.

An agent reading a document reads untrusted text. The local service executes code from anyone with editor access. On shared documents, that means running collaborators' code on your machine.
