# SPEC: `komodoc sync`, the file on disk as a peer in the session

Status: the server's side is built and the client is not. The room holds
the document as a `yrs::Doc`, speaks the protocol below, answers
`y-checkpoint`, and the word diff and three-way merge the client needs are
the `komodoc-text` crate in `text/`, with their tests. What is not built is
the command itself: the peer, the mirror, the `base` bookkeeping around the
merge, the lock file. Planned for the existing Rust command line with Tokio.

## The problem

A document published from markdown or typst can be edited in the page it is
read in, by its owner and the editors the owner names (`03-SPEC-sharing.md`),
from as many tabs and machines as they like at once:
the source is a Yjs document the server holds and relays, and what readers
see is that document as it stands (`01-SPEC-history.md`).
That editor is the only door into a live session. The author who wrote the
paper in vim, Positron or Emacs, and who renders it from a Makefile, has to
choose between their own tools and the session: publish from the command line
and the session is bypassed, or open the browser and leave the tools behind.

`komodoc sync` makes the file on disk a peer in the session. Run it on the
file a document was published from, and edits made in any local editor flow
into the session while a browser tab types in it, edits made in the browser
land in the file, and saving the file marks a checkpoint in the document's
history. The browser stays the tool
for simultaneous work and for whoever has no toolchain; the file stays the
tool for the author who has one. Neither is an import of the other.

This is also the first program that is not a browser to read and write the
shared document. It uses Yrs in the same Rust executable as the server,
while the browser continues to use Yjs. Compatibility is verified by
cross-language tests, not assumed from the shared protocol.
Whatever joins a session on behalf of an author later -- a language model
acting on comments, a batch job -- joins it the way this command does.

A note on names. "Agent" already means the script the server injects into
every document it serves (`web/src/agent`). This spec never uses the word
for the program described here; that program is the sync client.

## What it looks like

```sh
komodoc sync c9k paper.typ
```

The ID is the one `list` prints, the same as `comment`, `edit` and `export`
take. The file is the source the document was published from, or will be.
The command runs until interrupted, and says what it is doing:

```
syncing paper.typ with https://komodoc.arelbundock.com/docs/typst-what-a-confidence-interval-does-not-say-5vvxv8ebpd
joined the session (2 peers)
paper.typ changed: checkpoint 4f2a91c
session changed: wrote paper.typ
paper.typ changed: checkpoint 8b03d77
```

Flags:

| flag | meaning |
| --- | --- |
| `--server` | the deployment, as every other command takes it |
| `--interval 250ms` | how long the file has to stay quiet before it is read, and the session before it is written |

Writing the file asks the server for a checkpoint. In the browser nothing is
deliberate any more -- the document is always current, and history is taken on
the server's own schedule -- but on disk, writing the file is a deliberate act,
and a deliberate act is worth a mark in the timeline. The checkpoint is of the
session's text at that moment, which after reconciliation is the file's; the
server writes nothing if that text is already the latest checkpoint.

The same command on an HTML source is the case it was made for:

```sh
komodoc sync c9k paper.html
```

beside a Makefile that runs `quarto render` turns every render into a write
of the file, and so into a checkpoint, with no step between the author's
tools and the readers.

Only an editor of a document may edit its source, in the browser and here:
its owner, and whoever the owner has named (`03-SPEC-sharing.md`). The command
asks the document's endpoint for the caller's role before it starts and
refuses with a plain message below `editor`, because the server drops
anyone else's `y-*` messages silently and a sync client that ran anyway
would sit there doing nothing.

## What the server is

The server holds the document as a `yrs::Doc` in the room, persists it, and
answers `y-open` from it, so a session is never seeded by a peer and never
ends when the last one leaves. The sync client is therefore a peer among
peers and needs no new route. The protocol it speaks is the browser's, and
the server's side of every row is built:

| direction | message | meaning |
| --- | --- | --- |
| in | `hello {comments}` | on connect; the comments, which the client ignores |
| out | `y-open` | I am here |
| in | `y-state {update, count}` or `y-state {ref, count}` | the document as the server holds it, one encoded state; apply it. Above the socket message cap the state comes as a same-origin `ref` to fetch instead, after which the client sends its state vector as `y-sync` to catch up |
| out, in | `y-update {update, seq}` | one Yjs update, base64 |
| in | `y-ack {seq}` | the update is durable; until then it is the client's to resend on reconnect |
| out, in | `y-awareness {update}` | who is here, base64, relayed and never stored |
| in | `y-peers {count}` | how many sockets the room has |
| out | `y-checkpoint {why: "sync"}` | the file was written; take a checkpoint if the text has moved |
| in | `y-checkpoint {sha}` | the checkpoint was taken, possibly after the spacing `01-SPEC-history.md` imposes; print it |

Identity is the `Authorization: Bearer` header the command line already
sends, on `GET /ws/<slug>`. There is no source to fetch and no SHA to hold:
the document's text is what `y-state` carries. The updates are Yjs's
binary v1 encoding; the server uses Yrs, the browser Yjs, and
`komodoc/src/tests/yjs.rs` holds the interoperability tests the client
inherits.

## Design

### The peer

The sync client holds a `yrs::Doc` with one shared text named `source`,
the name `collab.js` uses, and compatible awareness state. A Tokio task
owns the document and serializes events from the WebSocket transport and
filesystem watcher; no Yrs transaction is held across an await. Configure
text offsets to match browser UTF-16 positions and test supplementary
Unicode characters explicitly.

On `y-state`, apply the state, then reconcile the file against the
document as a local change (below), so a file that has moved on since the
client last ran becomes an edit of the document rather than a second copy of
the same words. There is no seeding: the server has the document whether or
not anyone is editing it.

On `y-awareness`, apply it and print who joined or left. The client's own
awareness entry is
`{user: {name: "<login> (sync)", color}}`, so a caret label in the browser
says where the other edits are coming from, even though the client has no
caret to show.

If the socket drops, reconnect with backoff, send `y-open` again, and treat
what comes back as above. Reconnecting is the ordinary case for a process
that runs all day on a laptop that sleeps.

### The mirror

Session to disk. Every update from the socket marks the document dirty. When
it has been quiet for the interval, the text of `source` is written to a
temporary file beside the target and renamed over it, so an editor never
reads half a write and a crash never leaves half a file. The digest of what
was written is remembered, and the watcher ignores the event that write
causes. Permissions are copied from the file being replaced.

Disk to session. The file is watched with a Rust filesystem watcher on the
parent directory. When it has been quiet
for the interval, it is read, normalised (CRLF to LF, invalid UTF-8 refused
with a message), and compared by digest with what was last written. If it is
the same, nothing happened. If it differs, the change is reconciled into the
document -- and reconciled is not replaced. Replacing the whole text would
delete and reinsert every character, which destroys the other editors'
concurrent insertions, every caret position, and the anchors of every
comment. The document gets the smallest set of inserts and deletes that turn
its text into the file's, applied in one transaction, against the text as it
stood at the start of that transaction. Because one task does everything,
no remote update lands between reading the document and writing to it.

Then `y-checkpoint` is sent, debounced by the same interval. The server
already spaces requested checkpoints thirty seconds apart and answers with
the SHA when it takes one, so a burst of saves is one mark in the timeline
and an editor's auto-save is at most two a minute.

### The merge, which is the only hard part

A text editor is a snapshot client. It read the file at some moment, holds
the text in a buffer, and writes the buffer back whole when the author saves.
If the session moved meanwhile -- the same author in a browser tab, a sync
client on their other machine, a restore from the reader -- the buffer does
not know, the saved file lacks those words, and a diff of the document
against the file would remove them. That is the case the design has to get right,
and the rest is plumbing.

The fix is a three-way merge. The client keeps the **base**: the text the
file and the session last agreed on, which is what was last written to disk
or last read from it without conflict. On a file change:

1. `local` = what the file says now; `remote` = what the document says now.
2. If `remote == base`, nothing happened in the session: diff `base` to
   `local`, apply. This is the common case and it is exact.
3. Otherwise merge `base`, `local` and `remote`. Edits to different regions
   go through, and the merged text becomes the target: diff `remote` to
   `merged`, apply. The session's edits are already in `remote`, so they
   survive; the file's edits are in the diff, so they arrive.
4. Where both sides changed the same region, the **session wins**, the merged
   text is written back to the file, and a line is printed naming the region.
   The session is what every other peer and every reader is looking at, and
   the file is one buffer; and the file's author is looking at a terminal
   that just told them, while a browser tab would find out only by noticing
   words vanish.

Afterwards `base` is the document's text, whichever branch ran.

The merge is by word, not by line, and it is `komodoc_text::merge` in
`text/`, beside the `diff` the room's restore uses: disjoint edits, adjacent
edits, the same word on both sides, an insert at the seam of a deletion, an
edit at either end, an empty base, UTF-16 offsets and random soups are its
tests. What the client adds is the `base` bookkeeping above and the printed
line naming a conflict.

The other direction has no merge to do. When the session changes and the
file is written, the editor either reloads it or does not: VS Code and
Positron reload a clean buffer without asking, vim does with `autoread` on
its next focus or command, Emacs polls under `auto-revert-mode`. A buffer
that is dirty when the file changes gets whatever that editor does about a
file changed on disk, and that is the editor's prompt to give, not ours. What
the client guarantees is that the file on disk is never behind the session
by more than the interval.

### What the client does not render

The executable carries the engine, and an earlier draft had the client
render here, feeding includes, images and bibliographies from the directory
the file is in through the engine's file map. Nothing rendered is stored
any more, and
readers render for themselves, so there is nothing for the client to render
for. What that leaves exposed is a gap the rendering used to paper over: a
typst document that `#import`s a file or reads a `#bibliography` beside it
renders on this machine and nowhere else, because the engine's file map in
the browser has nothing to fill it. The browser editor has
always had this gap; readers now have it too. Closing it means the files
beside the source travelling with it -- a project rather than a file --
which remains future work and which this command can feed once it exists.

### A write from outside

Someone runs `komodoc publish` on the same slug, restores a checkpoint from
the reader, or types in a browser. Each is an edit into the one document,
and the server relays it to the client as a `y-update` like any other; the
mirror writes it to the file, the merge above keeps the author's unsaved
words, and nothing stops. The rule an earlier draft had here -- stop
publishing and wait for a restart -- existed because there were two copies
of the document to disagree. There is one.

## Follow-up: automation clients as peers

The peer module also backs a komodoc subcommand and small library for a
headless automation client. This follows the sync command and the scoped
credentials in `03-SPEC-sharing.md`; it is not another name for the annotation
script in `web/src/agent`.

The client joins the same Yjs session to read and edit source, and uses the
room's `comment`, `reply`, `resolve`, and `delete` messages for annotations.
Publish and version that protocol, including required fields, replies,
errors, and compatibility behavior, rather than making clients infer it
from an internal message struct. Do not introduce a separate text-replacement
API with different concurrency rules.

Provide an authenticated snapshot GET for the current source and annotations
so a client can inspect a document before joining. Its read authorization is
the same as the document's, including private visibility and token scope.
The snapshot is a read aid, not a substitute for the Yjs synchronization
handshake before writing. Keep source and annotation reads within one room
snapshot and include the source SHA so the client can identify what it saw.

The room persists edits under `01-SPEC-history.md`, even with no browser
attached. The older proposal that a peer render HTML for each save is
superseded: this client edits source and requests checkpoints just as sync
does. LaTeX rendering, when needed, follows `05-SPEC-latex.md` rather than
creating an automation-only server renderer.

Tests cover a browser and an automation client editing concurrently,
annotation creation and resolution with the token's author identity, a
snapshot refused outside its document scope, and edits surviving the last
headless peer leaving and the server restarting.

## Edge cases, and what is decided about each

- **The file does not exist.** Written from the document's text on start, and
  said so. This is how an author pulls a document down to edit locally.
- **No such document.** Refused: `sync` takes a document that exists,
  `publish` makes one. The message says which to run.
- **The editor's temporary files.** Vim's swap and backup files, Emacs's
  `#paper.typ#`, an editor's `paper.typ~`: the watcher is on the one path,
  and events for any other name are ignored.
- **Editors that write by rename.** Vim, by default, writes a new file and
  renames it over the old one, so the inode changes. the filesystem watcher reports that
  as a rename, or a remove and a create, depending on the platform. The
  watcher is on the parent directory, filtered to the name, so it survives.
- **A trailing newline.** Editors add one; the browser does not. The document
  is what it is: a newline the editor adds is an edit like any other, it
  goes into the session, and the browser shows it. Nothing normalises it
  away, because a rule that stripped it would fight the editor forever.
- **Format on save.** A formatter that rewrites the file touches every line,
  which is a diff against everyone and a re-anchor of every comment. Not
  prevented; documented as the thing to turn off for a synced file.
- **Two sync clients on one file.** Two people cannot have the same file,
  but one person can run the command twice. The second is refused by a lock
  file beside the target, with the pid of the first in it.
- **Two sync clients on one document, different machines.** Fine. Each is a
  peer; the session is the meeting point, as it is for two browsers.
- **The document is deleted while syncing.** The socket closes with the
  reason the room gives; the client prints it and exits, and leaves the file
  alone.
- **Binary assets, other files.** Out of scope. The session is one `Text`,
  and project-level asset transport is not built. The existing engine ABI
  can accept files, but the browser has no project assets to feed it. Images beside a typst file are not
  synced, and since nothing rendered is stored, nobody sees them until the
  file map exists; see "What the client does not render".

## What it is not

It is not live collaboration from vim. The local side of the session moves
when the file is written, and a file is written when the author saves -- or
every second or so with an editor's auto-save, which is the setting to
recommend. Two people typing in the same paragraph at the same time is the
browser editor's job, and the command's documentation says so in its first
paragraph.

It is not an editor plugin. A VS Code extension that made the editor buffer a
real Yjs peer, with the other peers' carets drawn as decorations, would remove
the snapshot problem entirely, and would cover Positron with the same code.
It is a different and larger project; this command is what it would be
measured against, and what it would replace only for people who ask.

It is not a general file synchroniser. One file, one document, one session.

## Steps

1. **The peer.** Yrs, a Tokio WebSocket client, the message types
   above, state on open, snapshot on request, awareness in and out,
   reconnect. A test runs the server in-process, joins two clients through
   it, and checks that an insert on one is the text of the other, that the
   second to join receives the state, and that a client which reconnects
   after the server restarts has lost nothing. Nothing touches disk yet.
2. **The mirror, one direction each.** Session to disk with atomic writes
   and echo suppression; disk to session as a minimal diff. Tests drive the
   watcher with real writes into a temporary directory and assert on the
   document; and drive the document and assert on the file.
3. **The merge, wired.** `komodoc_text::merge` in the disk-to-session path
   with `base` bookkeeping, and a test in which an actual Yjs browser peer
   edits while the file holds a stale copy.
4. **The checkpoint.** `y-checkpoint` sent after a file write, debounced. A
   test writes the file and checks that the manifest gains one entry whose
   bytes are the document's text, and that a second write of the same text
   gains none.
5. **The command.** The subcommand in the executable, ownership check, the
   lock file, the messages above, `--interval`. A section in the README
   under "CLI", after "Edit", which opens with what it is not.

Steps 1 and 2 are a day each; 3, 4 and 5 are a day or two together now
that the merge itself exists.

## Open questions

- **Cursor positions.** An editor that speaks LSP knows where its cursor is.
  A future sync client could take that over a local socket and put it in
  awareness, so the browser draws a caret for the file. Not now.

Two questions an earlier draft had here are answered in `01-SPEC-history.md`:
the session outlives every peer, because it is the document; and a write
from outside is merged, because there is nothing else it could be.
