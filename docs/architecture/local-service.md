---
title: "Local service and agents"
---

## Local service

The desktop app runs a loopback service, the `librepaper` binary in local mode (`librepaper local`, from the companion crate). It runs tools on the author's machine for the things a browser cannot do:

- Local builds: Quarto renders Quarto and Markdown to HTML, PDF and DOCX, Calepin renders Typst, and latexmk builds LaTeX PDF. Quarto and Calepin run the document's code.
- Zotero, read-only.

The protocol is [versioned](../dev/protocol/local.md). The current wire version is 3.2.0. It runs over loopback HTTP with bounded request bodies, uploads, file counts, PDF and log sizes, and job deadlines. The health endpoint identifies the service and negotiates the version. Given a challenge, it also answers with a proof keyed by the control token, so a process that took the port cannot pose as the service. Requests with a mismatched `Host` header are refused before anything is parsed. This blocks DNS rebinding.

The service writes a credential to `control-token.json` (mode 0600) in its state directory. The desktop app holds it. Browsers never call the local service directly.

- Every route except `GET /librepaper/local/health` requires `Authorization: Bearer <credential>`.
- The desktop app forwards each browser request through its `lp-local` scheme and sends the page origin as `X-LibrePaper-Origin`.
- A request with a browser `Origin` or `Sec-Fetch-Site` header gets 403.

The desktop app chooses the server. The default is `https://app.librepaper.org/`, and `LIBREPAPER_SERVER` selects a self-hosted server.

The desktop app trusts that server. The Run code button and switch are in the page, so the server origin could start a run without a press. A compromised server, or a script injected into its pages, can run code on the author's machine.

`librepaper local endpoint` starts the service if it is not running, then prints one line of JSON with the address and credential. The desktop app runs it at startup and again after a failed request. Users never start, stop or pair the service themselves. Folder choices use the operating system's native chooser. Folder names and contents stay local.

## Agents

An agent edits through MCP to the server. MCP owns the transport, and the room owns effects and durability. The caller supplies the source tree identity and the immutable byte ranges it is changing. The validator produces a new source, and patches whose ranges do not match are refused. A validated batch applies in one commit.

Agent work arrives as [tracked changes](review.html#track-changes), and editors accept or reject each one. An agent's authority is its link. The caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or a transcript store.

An agent reading a document reads untrusted text. The local service runs code from anyone with editor access, so on a shared document that means running collaborators' code on your machine.
