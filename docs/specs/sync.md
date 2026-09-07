# SPEC: sync presence and automation peers

“Agent” names the annotation script in `web/src/agent`. A program joining a
session is a peer: either the sync client or an automation client.

## The peer's name in the browser

Add the sync client's awareness entry, `{user: {name: "<login> (sync)", color}}`,
so the browser identifies where edits come from. Encode Yjs awareness in
Rust and expose presence without implying that the file client has a caret.

## Automation clients as peers

Extract a reusable peer module from `crates/komodoc/src/sync.rs` and use it
to back a komodoc subcommand and small library for a headless automation client. Its credential is a comment or edit link rather
than a person's sign-in, presented the way `--key` presents one; it is not
another name for the annotation script in `web/src/agent`.

The client joins the same Yjs session to read and edit source, and uses the
room's `comment`, `reply`, `resolve`, and `delete` messages for annotations.
Publish and version that protocol, including required fields, replies,
errors, and compatibility behavior, rather than making clients infer it
from an internal message struct. Do not introduce a separate text-replacement
API with different concurrency rules.

Provide an authenticated snapshot GET for the current source and annotations
so a client can inspect a document before joining. Its read authorization is
the same as the document's: its owner, and the role the link carries. The
snapshot is a read aid, not a substitute for the Yjs synchronization
handshake before writing. Keep source and annotation reads within one room
snapshot and include the source SHA so the client can identify what it saw.

The client edits source and requests checkpoints through the existing room,
including when no browser is attached. Rendering stays on clients; do not
introduce an automation-only server renderer.

Tests cover a browser and an automation client editing concurrently,
annotation creation and resolution attributed the way the link attributes --
to the signed-in account behind it, or to the link's pseudonym -- a snapshot
refused with a link minted for another document, and edits surviving the last
headless peer leaving and the server restarting.

## Cursor positions, later

A future sync client could receive cursor positions from an editor over a
local socket and publish them through awareness. This is deferred; the
presence entry does not require an editor plugin or LSP integration.
