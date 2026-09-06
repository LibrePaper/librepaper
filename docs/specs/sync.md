# SPEC: `komodoc sync`, the file on disk as a peer in the session

Status: built, in `crates/komodoc/src/sync.rs`, with `crates/komodoc/src/tests/sync.rs`
and the merge's own tests in `crates/text/src/tests.rs`. The command makes the
file a document was published from a peer in its live session: edits made in
any local editor flow into the session as the smallest diff, edits made in the
browser land in the file by atomic rename, a three-way merge by word keeps both
sides' words when a stale buffer is saved over a moved session, and every
write of the file asks the server for a checkpoint. Yrs is the library on both
sides of the Rust half and Yjs on the browser's, and the suite joins a Yrs
client to the real room over a real socket rather than assuming the shared
encoding. What remains is the awareness entry that would put a name on this
peer in the browser, and the automation client that joins a session the way
this command does.

A note on names. "Agent" already means the script the server injects into
every document it serves (`web/src/agent`). This spec never uses the word for
a program that joins a session; such a program is a peer, and the two kinds
are the sync client and the automation client.

## What the remaining work builds on

The sync client is a peer among peers and needs no new route. The protocol it
speaks is the browser's, every row is built on both sides, and an automation
client speaks the same one:

| direction | message | meaning |
| --- | --- | --- |
| in | `hello {comments}` | on connect; the comments, which the sync client ignores |
| out | `y-open` | I am here |
| in | `y-state {update, count}` or `y-state {ref, count}` | the document as the server holds it, one encoded state; apply it. Above the socket message cap the state comes as a same-origin `ref` to fetch instead, after which the client sends its state vector as `y-sync` to catch up |
| out, in | `y-update {update, seq}` | one Yjs update, base64 |
| in | `y-ack {seq}` | the update is durable; until then it is the client's to resend on reconnect |
| out, in | `y-awareness {update}` | who is here, base64, relayed and never stored |
| in | `y-peers {count}` | how many sockets the room has |
| out | `y-checkpoint {why: "sync"}` | the file was written; take a checkpoint if the text has moved |
| in | `y-checkpoint {sha}` | the checkpoint was taken, possibly after the spacing `docs/specs/history.md` imposes; print it |

Identity is the `Authorization: Bearer` header the command line already
sends, on `GET /ws/<slug>`, or the link key `--key` sends as
`x-komodoc-key`. The server drops the `y-*` messages of anyone below
`editor` silently, so a client asks the document's endpoint for its role
before it starts and refuses with a plain message rather than sitting there
doing nothing. The updates are Yjs's binary
v1 encoding; `crates/komodoc/src/tests/yjs.rs` holds the interoperability
tests any Rust peer inherits.

The client holds a `yrs::Doc` with one shared text named `source`, the name
`collab.js` uses, with text offsets in UTF-16 to match the browser. One Tokio
task owns the document and serialises the socket and the watcher, so no Yrs
transaction is held across an await and no remote update lands between
reading the document and writing to it. On `y-snapshot` it encodes the full
state as a v1 update with `replace: true`, which is what the browser does for
an older server. If the socket drops it reconnects with backoff and sends
`y-open` again. All of this is what an automation client reuses.

## The peer's name in the browser

Not built: the client's own awareness entry, `{user: {name: "<login>
(sync)", color}}`, so a caret label in the browser said where the other edits
were coming from. Awareness is who is here now, this client has no caret to
show, and the entry means encoding `Y.Awareness` in Rust for one label. It is
worth doing and it is not the command.

## Automation clients as peers

The peer module also backs a komodoc subcommand and small library for a
headless automation client. Its credential is a comment or edit link rather
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

The room persists edits under `docs/specs/history.md`, even with no browser
attached. The older proposal that a peer render HTML for each save is
superseded: this client edits source and requests checkpoints just as sync
does. LaTeX rendering, when needed, follows `docs/specs/latex.md` rather than
creating an automation-only server renderer.

Tests cover a browser and an automation client editing concurrently,
annotation creation and resolution attributed the way the link attributes --
to the signed-in account behind it, or to the link's pseudonym -- a snapshot
refused with a link minted for another document, and edits surviving the last
headless peer leaving and the server restarting.

## What it is not

It is not live collaboration from vim. The local side of the session moves
when the file is written, and a file is written when the author saves -- or
every second or so with an editor's auto-save, which is the setting to
recommend. Two people typing in the same paragraph at the same time is the
browser editor's job.

It is not an editor plugin. A VS Code extension that made the editor buffer a
real Yjs peer, with the other peers' carets drawn as decorations, would remove
the snapshot problem entirely, and would cover Positron with the same code.
It is a different and larger project; this command is what it would be
measured against, and what it would replace only for people who ask.

It is not a general file synchroniser. One file, one document, one session.

## Open questions

- **Cursor positions.** An editor that speaks LSP knows where its cursor is.
  A future sync client could take that over a local socket and put it in
  awareness, so the browser draws a caret for the file. Not now.
