# SPEC: history, as a document that is never lost

Status: the foundation and the timeline are built and are no longer
described here. The server holds the document as a `yrs::Doc`, persists it,
takes checkpoints, and serves readers who render from the live source;
`crates/komodoc/src/session.rs`, `crates/komodoc/src/room.rs`, `crates/komodoc/src/history.rs`
and `web/src/lib/collab.js` are the record of it. The timeline panel, the
checkpoint fields on a comment and the response export are
`crates/komodoc/src/export.rs`, `web/src/lib/history.js`,
`web/src/lib/passages.js` and `web/src/components/History.svelte`, with
`crates/komodoc/src/tests/timeline.rs` and the two checks under `web/checks/`.
What remains is steps 8 to 10 below: the word-level diff in the browser,
restore, and git provenance. Each of them reads the manifest the server
already writes.

## What the remaining work builds on

The parts of what is built that the steps below depend on, kept here
because other specs cite them.

| key | contents | who may read |
| --- | --- | --- |
| `sessions/<slug>` | the Yjs state of the live document, as one update, replaced on a debounce and at every checkpoint | the server |
| `history/<slug>/index.json` | the manifest: the list of checkpoints, oldest first | anyone who may read the document |
| `history/<slug>/<sha>` | one checkpoint: the source bytes | anyone who may read the document |

A manifest entry:

```json
{
  "sha": "4f2a91c…",
  "parent": "8b03d77…",
  "at": "2026-09-05T14:02:11Z",
  "by": "vincentarelbundock",
  "why": "quiet",
  "source_format": "typst",
  "size": 61240,
  "label": "sent to the journal",
  "commit": "a1b2c3d…",
  "dirty": false
}
```

`why` is one of `quiet`, `left`, `comment`, `cli`, `sync`, `restore`,
`label`, and `recovered` for one the manifest lost and a later checkpoint
found. `label` is set by a `PATCH` to the entry, from the panel or `komodoc
label`; `commit` and `dirty` are recorded as sent and checked only for
shape, and step 10 is what sends them. `changed` lists the paths whose
digest differs from the parent's. The chain is linear: a restore has the
current checkpoint as `parent` and an old one's bytes.

`GET /api/documents/<slug>/history` returns the manifest and `GET
.../history/<sha>` a checkpoint, to anyone who may read the document;
`komodoc history` prints the one and the panel fetches the other on demand.
Over the socket, `y-checkpoint` with a `why` of `sync`, `restore` or
`label` takes a checkpoint on request and answers with its SHA, and
`Room::restore` diffs a checkpoint's tree into the live Yjs document so a
tab typing at that moment keeps its words; restore has no route, command or
button yet.

The order of writes at a checkpoint is the checkpoint object, then the
session state, then the index entry, then the manifest, so that a manifest
missing its newest entry is repaired by the next checkpoint. A request for a
checkpoint from outside the room is deferred if one was taken within the last
thirty seconds, so a burst of saves is one mark. Quota is shed from the
oldest unlabelled checkpoint, then the oldest labelled, never the newest.

A comment carries `revision`, the SHA of the checkpoint it was made on, and
`resolved_in`, the SHA current when it was resolved, both set by the server
and both in the exports. With them the browser looks a comment's passage up
in any checkpoint's visible text, the way the agent takes it, and follows it
forward to the checkpoint where it stops being found. That lookup is what
the card's "then and now" line and the response export run on, and it
answers whether a passage is still there and when it went. It does not say
what replaced it: that is the word-level diff, and it is step 8.

## What is built on it

### What changed since

A reader picks a checkpoint -- by default the one their last comment was
made on, or the last one they opened, which the browser remembers locally --
and the reader lists what changed between it and now, as passages rather
than as a source diff. Each entry is a hunk of the word-level diff of the two
visible texts, with a few words of context either side. A changed or
inserted passage anchors into the current document by the same quotation
mechanism a comment uses, so clicking it reveals the place; a deleted
passage cannot anchor and is shown in the list with the words that survived
on either side of it.

This is the same diff `docs/specs/sync.md` needs for its three-way merge, by word
because paragraphs are single lines. The list is a component beside the
comments, not a rendering of insertions and deletions inside the document:
painting a diff into rendered HTML is a research problem and painting it
into a typst document is not possible, whereas anchoring a quotation is a
thing the reader does already.

The same diff closes two gaps the timeline left. The panel lists each
entry's `changed` paths but puts nothing behind them; a per-file diff goes
there. And the comment card and the response say a passage is gone, and
when, but not what stands in its place; with the diff, the "now" line can
quote the replacement.

### Diff and restore

In the editor, a merge view between any two checkpoints' sources, which
CodeMirror provides as a package with per-hunk accept and reject, so an
author can pull a sentence back from an old draft without leaving the page.
`komodoc diff c9k 8b03d77 4f2a91c` prints the same as a unified diff of the
sources, for a terminal or a pipe.

Restore, from the panel or `komodoc restore c9k 8b03d77`, puts that
checkpoint's tree into the live session through `Room::restore`, and takes
a checkpoint with `why: restore`. History is never rewritten; the checkpoint
restored from is still there, and so is the one restored over. The bar the
panel shows over an earlier version gains "Restore" beside "Name this
point", and a "Copy link" to that checkpoint.

### Pinning, later

The one thing lost by making readers see the current text is working on a
draft while readers keep seeing the old one. The answer is not a return to
publishing but a per-document setting, for editors (`docs/specs/sharing.md`
gives it to them, as a decision about the text): "readers see this
checkpoint", after which the reader page shows that checkpoint to everyone
but the editors, with a line saying a newer text exists, until an editor
unpins or pins a later one. The history panel is where it is set. It is
left out because it is a policy on top of the timeline, and because it may
turn out that nobody asks for it.

### The latest checkpoint that compiles

A reader who joins a document that does not compile is shown the engine's
diagnostics page. The better answer is the latest checkpoint that compiles,
which the panel can now find: the manifest is in the browser, and a
checkpoint is one fetch.

## What it is not

It is not keystroke history. The Yjs log between checkpoints exists and is
persisted, and with garbage collection off it would give any past state and
per-character attribution, which is Overleaf's premium feature. It is rolled
up on the snapshot schedule instead, because it grows with every keystroke
and deletion, and because a checkpoint's `by` already says who was editing
at the grain that matters. `docs/specs/sharing.md` gives a document editors with
identities of their own; if per-character attribution is ever wanted,
keeping the log is a flag, and it would sit beside this spec, not replace
it.

It is not a store of rendered pages, ever. If an operator wants the rendered
form of an HTML document kept when its source is regenerated -- a notebook
whose figures changed -- that is a new checkpoint of the `html` source,
which is what the format means.

It is not a git remote. Provenance is a commit hash recorded on a
checkpoint, so the author can find it; it is not a way to push or pull.

## Steps

8. **What changed since.** The word-level diff's WASM export from the
   engine, the list beside the comments, anchoring hunks by quotation, the
   per-file diff behind a changed path in the panel, and the replacement
   text on the card and in the response.
9. **Diff and restore.** The merge view in the editor, `komodoc diff`,
   `komodoc restore`, the route and the panel's "Restore" that call
   `Room::restore`, and "Copy link" on a checkpoint.
10. **Provenance.** `publish` and `sync` send the git fields.

8 and 9 are the browser work and take longer. 10 can go anywhere.

Git provenance, in step 10: when `publish` or `sync` runs inside a git
repository, the checkpoint it causes records the commit the working tree was
at and whether it was dirty. This is one process spawn, and it is the pointer
into the history the author's files already have, which for a document
written in Quarto is the only source history there is. `--no-git` leaves it
out.

## Open questions

- **The source is public to readers**, which is the honest price of storing
  nothing rendered. Who a reader is is a question `docs/specs/sharing.md`
  answers: "anyone who may read the document" in the storage table means the
  reader role there: the owner, and whoever holds a live link.
- **Rendering on the server.** Excluded from this plan. Having the native
  engine in the executable does not put compilation in the request path.
  Rendering and visual diffs run on clients; the server handles permission
  checks, synchronization, persistence, quotas, and retention. LaTeX
  readers use the client-produced PDFs described in `docs/specs/latex.md`.
- **The quiet interval.** Five minutes is a guess in both directions: long
  enough that a paragraph is a checkpoint and not each sentence of it,
  short enough that a session's worth of work is many points rather than
  one. The last-editor rule bounds the loss in any case.
