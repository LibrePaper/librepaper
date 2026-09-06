# SPEC: history, as a document that is never lost

Status: the foundation is built and is no longer described here. The server
holds the document as a `yrs::Doc`, persists it, takes checkpoints, and serves
readers who render from the live source; `komodoc/src/session.rs`,
`komodoc/src/room.rs`, `komodoc/src/history.rs` and `web/src/lib/collab.js` are
the record of it, with the tests in `komodoc/src/tests/` and `make smoke`.
The timeline is built too, and so are the checkpoint fields on a comment and
the response export -- steps 5 to 7, whose record is `komodoc/src/export.rs`,
`web/src/lib/history.js`, `web/src/lib/passages.js` and
`web/src/components/History.svelte`, with `komodoc/src/tests/timeline.rs` and
the two checks under `web/scripts/`. What remains is steps 8 to 10 below: the
word-level diff, restore, and git provenance. Each of them reads the manifest
the server already writes.

## What the remaining work builds on

The parts of the built foundation the steps below depend on, kept here
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
`label`. `label` is empty until somebody sets one, and `commit` and `dirty`
are the git provenance step 10 fills in. The chain is linear: a restore has
the current checkpoint as `parent` and an old one's bytes.

The order of writes at a checkpoint is the checkpoint object, then the
session state, then the index entry, then the manifest, so that a manifest
missing its newest entry is repaired by the next checkpoint. A request for a
checkpoint from outside the room is deferred if one was taken within the last
thirty seconds, so a burst of saves is one mark. Quota is shed from the
oldest unlabelled checkpoint, then the oldest labelled, never the newest.

## Comments know their checkpoint

Two fields on `Comment`, both set by the server in `apply`, never by the
client:

- `revision`: the SHA of the checkpoint the comment was made on, which is
  the checkpoint `apply` takes or finds current.
- `resolved_in`: the SHA current when it was resolved, alongside
  `resolved_at`.

A comment from before the fields existed has neither, and is treated as made
on the oldest checkpoint the manifest knows. Both fields travel in the
JSON-LD export as extra properties, which the Web Annotation model permits
and the export already relies on for `resolved`.

With these, a comment's passage can be looked up in any checkpoint's text
by the match that anchors it in the reader: the browser renders the
checkpoint, takes the visible text the way the agent does, and searches it.
Found at its own checkpoint, followed forward until the checkpoint where it
stops being found, and quoted from the current text if it is still there.
That lookup is the primitive under everything in the next section, and it
runs entirely in the browser, from the manifest and the checkpoints it
fetches on demand.

## What is built on it

In the order they are worth having.

### The timeline

`GET /api/documents/<slug>/history` returns the manifest. `komodoc history
c9k` prints it:

```
sha      at                    by                  why      label
8b03d77  2026-09-03 09:12:40   vincentarelbundock  cli
4f2a91c  2026-09-05 14:02:11   vincentarelbundock  sync     sent to the journal
c07e1aa  2026-09-05 16:40:03   annegrandchamp      comment
d1e0f42  2026-09-05 17:02:19   vincentarelbundock  left     *
```

In the reader, a `history` button in the toolbar, in reading and editing
alike, opens a panel listing checkpoints by day, newest first, with the
author and the reason, labelled ones standing out, and runs of unlabelled
checkpoints by one person folded to their first and last. Selecting one
shows it in the document pane, read-only, with a bar saying which
checkpoint is showing and offering "Back to now", "Restore", "Name this
point" and "Copy link". A label is a `PATCH` to the manifest entry, for
editors, from the panel or from `komodoc label c9k 4f2a91c "sent to the
journal"`.

### The passage, then and now

Every comment card gains one line when the passage has changed since the
comment was made: what it said then, what it says now, or that it is gone,
with the checkpoint where it went. This replaces the badge as the answer to
"Needs re-anchoring": the comment is still shown as needing a home, but the
reader can see what happened to the passage instead of guessing. A document
with no changed passages costs one request for the manifest and nothing
more.

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

This is the same diff `04-SPEC-sync.md` needs for its three-way merge, by word
because paragraphs are single lines. The list is a component beside the
comments, not a rendering of insertions and deletions inside the document:
painting a diff into rendered HTML is a research problem and painting it
into a typst document is not possible, whereas anchoring a quotation is a
thing the reader does already.

### The response to reviewers

```sh
komodoc export c9k --format response --since 4f2a91c
```

For every comment made at or after the given checkpoint -- all of them
without `--since` -- a section with the passage as the reviewer saw it, the
thread, and the passage as it now stands or the note that it was removed,
and for a resolved comment the checkpoint it was resolved in. Grouped by
reviewer, because that is how a response is organised, and in the markdown
the `export` command already writes, so it goes into a Quarto document as it
is:

```markdown
## Reviewer: annegrandchamp

### 1. commenting, resolved in d1e0f42

> The confidence interval does not say that the parameter is inside it with 95% probability.

**Then:** "…with 95% probability, the true value lies in the interval…"

**Now:** "…95% of intervals built this way, over repeated samples, cover the true value…"

Fixed as suggested; see also the new footnote on coverage.
```

The last line is the author's reply from the thread, which is where the
response gets written from now on: replying to the comment in the reader is
writing the response document. That is the feature to show first, because
nobody else can build it -- their comments do not live on the text a reader
was shown -- and because it is cheap: one export format over the lookup
above.

### Diff and restore

In the editor, a merge view between any two checkpoints' sources, which
CodeMirror provides as a package with per-hunk accept and reject, so an
author can pull a sentence back from an old draft without leaving the page.
`komodoc diff c9k 8b03d77 4f2a91c` prints the same as a unified diff of the
sources, for a terminal or a pipe.

Restore, from the panel or `komodoc restore c9k 8b03d77`, writes that
checkpoint's source into the live session as an edit: the server diffs it
against the current text and applies the difference to the Yjs document, so
an owner's tab typing at that moment keeps its words and sees the rest
change under them. It takes a checkpoint with `why: restore`. History is
never rewritten; the checkpoint restored from is still there, and so is the
one restored over.

### Pinning, later

The one thing lost by making readers see the current text is working on a
draft while readers keep seeing the old one. The answer is not a return to
publishing but a per-document setting, for editors (`03-SPEC-sharing.md`
gives it to them, as a decision about the text): "readers see this
checkpoint", after which the reader page shows that checkpoint to everyone
but the editors, with a line saying a newer text exists, until an editor
unpins or pins a later one. The history panel is where it is set. It is
left out of the first version because it is a policy on top of the timeline
and needs the timeline first; and because it may turn out that nobody asks
for it.

## What changes elsewhere

**In the reader.** The toolbar gains `history`, while editing and while
reading alike, and the panel behind it.

**In the README.** A paragraph about the timeline goes in.

## What it is not

It is not keystroke history. The Yjs log between checkpoints exists and is
persisted, and with garbage collection off it would give any past state and
per-character attribution, which is Overleaf's premium feature. It is rolled
up on the snapshot schedule instead, because it grows with every keystroke
and deletion, and because a checkpoint's `by` already says who was editing
at the grain that matters. `03-SPEC-sharing.md` gives a document editors with
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

5 to 7 are built: the timeline and its panel, the two checkpoint fields on a
comment, and the response export. Two things the spec asked for above are not
in them, and both wait on step 8. The panel lists a checkpoint but does not put
a per-file diff behind each changed path, and neither the comment card nor the
response says what replaced a passage -- only whether it is still there, and
the moment it stopped being there, which is what the anchoring can answer on
its own.

8. **What changed since.** The word-level diff is built, as the
   `komodoc-text` crate (`text/`), which `komodoc sync`'s merge also uses;
   what remains is its WASM export from the engine, the list beside the
   comments, and anchoring hunks by quotation.
9. **Diff and restore.** The merge view in the editor, `komodoc diff`,
   `komodoc restore`, restore as a server-side diff into the document.
10. **Provenance.** The git fields from `publish` and `sync`.

8 and 9 are the browser work and take longer. 10 can go anywhere.

Git provenance, in step 10: when `publish` or `sync` runs inside a git
repository, the checkpoint it causes records the commit the working tree was
at and whether it was dirty. This is one process spawn, and it is the pointer
into the history the author's files already have, which for a document
written in Quarto is the only source history there is. `--no-git` leaves it
out. The server records what it is sent and checks only that a commit is
forty hex characters.

## Open questions

- **The source is public to readers**, which is the honest price of storing
  nothing rendered. Who a reader is is a question `03-SPEC-sharing.md`
  answers: "anyone who may read the document" in the storage table means the
  reader role there, and a `private` document has named readers only.
- **Rendering on the server.** Excluded from this plan. Having the native
  engine in the executable does not put compilation in the request path.
  Rendering and visual diffs run on clients; the server handles permission
  checks, synchronization, persistence, quotas, and retention. LaTeX
  readers use the client-produced PDFs described in `05-SPEC-latex.md`.
- **The quiet interval.** Five minutes is a guess in both directions: long
  enough that a paragraph is a checkpoint and not each sentence of it,
  short enough that a session's worth of work is many points rather than
  one. The last-editor rule bounds the loss in any case.

## Waiting on the timeline

A reader who joins a document that does not compile is shown the engine's
diagnostics page. The better answer is the latest checkpoint that compiles,
and it waits on step 5 to expose the manifest to the browser.
