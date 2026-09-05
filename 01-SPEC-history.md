# SPEC: history, as a document that is never lost

Status: steps 1 to 4 are built on the Rust host; steps 5 to 10 are not. See
"What was built" at the end, which also records what Yrs turned out to need
before it could be trusted against browser Yjs. This replaces an earlier draft of
the same spec, which kept history at the grain of a publish. There is no
separate publication copy in this one. It extends the existing Rust server
with a persistent shared document; the browser remains a Yjs client.
`04-SPEC-sync.md` is written against this protocol.

## Implementation order

The numbered specs order the remaining implementation work, with HTML kept
last as an already implemented reference rather than a pending milestone:

1. This spec, steps 1–4: durable editing, checkpoints, and reading from source.
   Reliability and Yrs/Yjs integration tests are part of this foundation.
2. `02-SPEC-diagnostics.md`, step 6: reader rendering failures.
3. `03-SPEC-sharing.md`, steps 2–6: grants, sharing UI, and visibility.
4. Return to this spec for steps 5–10: timeline, provenance, export, and diffs.
5. `04-SPEC-sync.md`: local file synchronization, then the automation
   follow-ups in sharing and sync.
6. `05-SPEC-latex.md`: browser compilation and PDF annotation, then SyncTeX
   and optional CLI compilation. Feasibility experiments may start earlier.

`06-SPEC-html.md` and diagnostics steps 1–5 are already built; preserve them
throughout. Files under `market-research/` are research references, not
implementation milestones.

## Host and cost constraints

Keep `komodoc/` as the Rust server and CLI, `engine/` as the native/WASM
renderer, and `web/` as the browser application. No host rewrite or
Cloudflare deployment is required. The browser renders immediately from
its local document, with expensive compilation in browser workers and
immutable renderer assets cached by digest. The server never compiles a
document in response to an edit or a read.

Yrs is a candidate, not an accepted implementation dependency. It would let
the Rust server and CLI hold the shared document without a JavaScript
runtime or a sidecar, but introduces a second CRDT implementation alongside
browser Yjs and requires compatibility testing. Neither faster previews nor
lower hosting costs have been established as benefits of this choice.
References to Yrs below describe that candidate implementation. Before
committing to it, pin versions and test real Yjs-to-Yrs exchanges: v1
updates, state vectors, deletions, concurrent edits, awareness, Unicode
UTF-16 positions, and reconnect after restart. Use a room task to serialize
mutations; do not hold a Yrs transaction across an await.
If this validation fails, resolve the CRDT integration separately; it does
not authorize a host rewrite or change the client-side rendering goal.

Retain the filesystem and S3 blob adapters, conditional index writes,
prefix isolation, and `/api/config` for deployment-specific limits and
enabled renderers. Bucket credentials stay on the server. Browser-held
credentials and a Worker-specific S3 adapter are outside this plan.
Multiple processes sharing a bucket need enforced exclusive ownership or
renewable, fenced writer leases; an in-process lock alone is insufficient.
Quota admission, checkpoint manifests, and deletion need a recoverable
commit sequence across room state and the index, with failure-injection
tests. A per-document lock does not serialize deployment-wide quotas.

Keep incremental socket updates bounded. For a full state larger than the
socket cap, `y-state` supplies an authenticated same-origin HTTP state
reference instead of inline updates; both paths encode the same document.
Catch up updates arriving during the fetch through state-vector exchange.
Apply the document's read permissions to this endpoint and enforce request
and decoded-state limits. Test near-limit HTML and reconnect on both paths.

Bound write batching, active rooms, per-peer queues, and update rates so
storage quotas are not mistaken for a bound on server CPU or traffic.
Persist before eviction, and measure bytes relayed, persistence writes,
and room memory with realistic sessions. Client-local edits never wait on
these operations to appear in the preview.

## The problem

A document has one version. `put` writes the new HTML, names it in the index,
and prunes every other object under `documents/<slug>/`; the source lives at
one unversioned key, `sources/<slug>`, overwritten on every save. The only
trace a revision leaves is `updated_at`, and the only memory of what the
text used to say is inside the comments: each one quotes the passage it was
made on, which is how it re-anchors when the passage moves, and why it gets
the badge "Needs re-anchoring" when the passage is gone.

That badge is the whole of what a reviewer gets today about a change. They
come back to a link a week later, and the document is different; which
sentences changed, whether their comment was acted on, and what the passage
they objected to became are all questions the document cannot answer. The
author is in the same position from the other side: at the end of a
revision cycle a journal wants a response to reviewers, and it is written
by hand from memory, one comment at a time.

The editor has the mirror-image problem. What is typed lives in a Yjs
session the server relays and forgets: `end_editing` clears the log when the
last socket closes, so a draft exists only in open tabs, and a tab closed by
mistake, a laptop that dies, or a browser that restarts loses it. The save
button, the "unsaved changes" badge, the `beforeunload` prompt, Ctrl-S, the
`base_sha` check and the 409 that says "reload before saving over it" are all
consequences of one fact: the durable copy of the document and the thing
being edited are two different objects, and the reader is asked to carry one
to the other by hand.

Overleaf sells history as a premium feature: a timeline, a diff between two
points, labelled versions, restore. typst.app has none, and delegates it to
git (`SPEC-typst-app-research.md`, §5, calls this the clearest gap). Both
keep history of the source, for the people editing it. Neither connects it
to comments, because in both the comments live in the editor, not on the
text a reader was shown. That connection is the thing to build, and it is
easier to build once there is only one object.

## The decision: one document, always current

Four rules. Everything below follows from them.

**The session is the document.** The Yjs document the room holds is the
document, not a draft of it. It is persisted by the server and outlives
every socket. It is seeded once, from the source the document was created
with, and never seeded again. The session outlives every client, the
browser as much as the sync client. Whoever opens the document, in whatever
tool, joins what is there.

**Readers see the current text.** A reader's browser joins the session
read-only, receives updates as they happen, and renders the text with the
engine the editor already loads, after a pause long enough that a word is
never shown half-typed. Reading and editing are the same page with the
source pane folded away. There is no moment at which the author decides
that readers may now see what was typed; the answer to "when do readers see
it" is "now", and the answer to "what if I am not ready" is a pinned
checkpoint, below.

**Nothing derived is stored.** The server keeps the source and nothing
rendered from it. Every browser that shows the document renders it, from
the live text or from a checkpoint, with the same module the editor
previews with, so what is shown is by construction what the source says.
This removes `documents/<slug>/<sha>.html`, `prune_other_versions`, the
`html` field of a publish, `max_html` as a ceiling on anything but the
source, and the whole class of "the HTML and the source disagree". A
document published as HTML from Quarto or a notebook has HTML for its
source, format `html`, and is stored and shown as such; it is not derived
from anything the store can see, and it is edited like the other two
(`06-SPEC-html.md`).

**There is no save.** Nothing a reader does makes the document durable,
because it always is. History is a sequence of **checkpoints** the server
takes on its own, at moments that mean something, and the only deliberate
act left is naming one. Restore is an edit. Nothing is ever rewritten, and
nothing is removed except by `destroy`, by expiry, and by the two ceilings
below, which shed the oldest checkpoints and never the newest.

## What a checkpoint is

A checkpoint is the source of the document at one moment, named by the
sha256 of its bytes. The server takes one:

- **After quiet.** When the session has had no update for `--checkpoint
  5m`, and the text differs from the last checkpoint.
- **When the last editor leaves.** The last editor socket closing, if the
  text differs from the last checkpoint. This is the one that replaces
  `end_editing`'s forgetting.
- **When a comment is made.** If the text differs from the last checkpoint,
  a checkpoint is taken before the comment is stored, so that every comment
  sits on a checkpoint by construction: what the reviewer saw is on record
  the moment they say something about it.
- **On a write from outside.** `komodoc publish` on an existing document,
  a file written under `komodoc sync`, and a restore each ask for one,
  because each is a deliberate act by the author, and a deliberate act is
  worth a mark in the timeline. Editors only, as a `y-checkpoint` message on
  the socket or implied by the route. A request that arrives within thirty
  seconds of the last checkpoint is deferred until the thirty have passed,
  and taken then if the text still differs: a burst of saves is one mark,
  and a sync client writing all day is two marks a minute at most. The
  thirty seconds are a constant in the Rust configuration module, not a flag. When the
  server takes a requested checkpoint it answers the requester with
  `y-checkpoint {sha}`, which is how `komodoc sync` knows what to print.
- **When somebody names the moment.** "Name this point" in the reader or
  `komodoc label` takes a checkpoint if the current text is not already one,
  then labels it.

A checkpoint whose SHA is already in the manifest is not written again and
adds no entry: quiet after quiet costs nothing. Two checkpoints by the same
author within a short span are still two checkpoints; the timeline in the
reader shows them folded, and folding is a matter of display, never of
storage.

Between checkpoints the live document is the only record. The server holds
it as a `yrs::Doc`, applies authorized updates, and batches encoded-state
writes on a bounded debounce and at every checkpoint. Relaying an update
is not an acknowledgment of durability: acknowledge a batch only after
storage succeeds. Clients retain unacknowledged updates locally and resend
them on reconnect, using state-vector synchronization. A crash may discard
an unacknowledged server batch; it must never discard an acknowledged one.
The browser reports pending persistence separately from connectivity and
must not claim work is saved merely because the socket is open. Checkpoints
are historical snapshots, separate from durability acknowledgments. The
server encodes its own state without asking a peer for `y-snapshot`.

## Storage

Three kinds of object. `sources/<slug>` goes away in favour of the first;
nothing may live under `sources/<slug>/` on the directory store, and there
is no longer a reason to want to.

This also replaces the earlier proposal to flush a relayed CRDT into
`sources/<slug>` when the last peer leaves. Persistence belongs to the room,
whether its editors are browsers, sync clients, or automation clients. No
browser must remain open to save another client's work, and no client must
submit rendered HTML to make a source edit durable. Steps 1 and 2 must test
a headless client's edit, disconnect, and subsequent server restart, with
the source and its checkpoint intact.

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

`parent` is the checkpoint before it. The chain is linear because there is
one document and every write goes into it; a restore has the current
checkpoint as `parent` and an old one's bytes, and says so in `why`. `why`
is one of `quiet`, `left`, `comment`, `cli`, `sync`, `restore`, `label`.
`by` is, for `quiet`, the editor whose update last arrived before the
quiet, and for `left`, the editor who left; a document's editors are its
owner and whoever the owner names (`03-SPEC-sharing.md`), and until that spec
lands the owner alone. For `comment` it is the commenter. `commit` and
`dirty` are git provenance, below. `label` is empty until somebody sets one.

The index entry for a document keeps `sha`, which now names the latest
checkpoint rather than an HTML object, and gains nothing else; the reader
asks the manifest for the rest.

Order of writes when a checkpoint is taken, so that a crash leaves nothing
worse than an untidy history: the checkpoint object, then the session
state, then the index entry, then the manifest. A manifest missing its
newest entry is repaired by the next checkpoint, which names the missing
one as `parent` and finds the object present.

`remove` deletes `sessions/<slug>` and `history/<slug>/` along with
everything else, which is what `destroy` has promised in the README since
before there was a history to delete.

### What it costs, and how it is bounded

A checkpoint is the source: tens of kilobytes for a paper, before the store
compresses anything. Two hours of writing at one checkpoint per five quiet
minutes is at most twenty-four of them, and in practice far fewer, because
quiet after quiet is not written. A document with a hundred checkpoints is a
few megabytes, on a store that charges a cent and a half per gigabyte-month.
No HTML is stored at all, which on a typst document with figures is the
saving that pays for all of this.

History is counted where the quotas look: the index entry's `size` is the
session state plus every checkpoint object, and `admit` enforces the
per-owner and total ceilings against it. A checkpoint is never refused,
because refusing it would lose work, which is the one thing this spec
exists to prevent; instead a checkpoint that would carry the document over
its ceiling is taken, and then the oldest unlabelled checkpoints are shed,
and the oldest labelled ones after them, until the document fits, exactly
as `--history N` sheds them below. Only when the session state and the
newest checkpoint alone exceed the ceiling is there nothing left to shed,
and then the reader is told the document is over its quota and the fix is a
person's. The cap on a document's size applies to the source in the session:
an update that would carry the text past it is not applied and not relayed,
and the socket that sent it is closed with the reason, so the room's text
never exceeds what a publish could have sent. So the bound on the bill is
the bound it was: the per-owner and total quotas, the size cap, and in the
sandbox expiry, which takes a document's session and history with it.

`--history N` caps checkpoints per document for an operator who wants one:
when a checkpoint would exceed it, the oldest unlabelled one is dropped, and
the oldest labelled one only when every checkpoint is labelled. The default
is no cap. `--history 0` keeps no history beyond the session state, which is
today's behaviour with the losing removed.

### Git provenance

When `publish` or `sync` runs inside a git repository, the checkpoint it
causes records the commit the working tree was at and whether it was dirty.
This is one process spawn, and it is the pointer into the history the
author's files already have, which for a document written in Quarto is the
only source history there is. `--no-git` leaves it out. The server records
what it is sent and checks only that a commit is forty hex characters.

## Reading without stored HTML

The reader today loads `/raw/<slug>/<sha>.html` into a frame on the
documents origin, and the agent injected into that page reports its text
back for anchoring. The frame stays, and so does its origin: it is what
confines a document that turns out to be hostile, and the agent is the only
thing on either side that touches the DOM. What changes is where the HTML
comes from.

`/raw/<slug>/` on the documents origin serves an empty shell with the agent
in it. The reader joins the session, renders the text with the engine, and
sends the page to the frame with the `preview` message the editor already
uses on every keystroke. A reader renders after a second of quiet; the
editor after sixty milliseconds, as now. A checkpoint is shown the same way,
from bytes fetched at `history/<slug>/<sha>`. A document whose format is
`html` is sent as it is.

Two consequences, one of them a change of policy.

**The source becomes readable by anyone who may read the document.** It
has to be: the browser cannot render what it is not given. Today the source
is owner-only and readers see only HTML, which for markdown is a distinction
without a difference, and for typst hides source comments, unused
definitions and whatever else the author left in the file. This spec accepts
that, and offers no way around it: publishing as HTML hides nothing, since
the HTML is the bytes every reader downloads (`06-SPEC-html.md`). A source that
must not be seen is not published here as that source.

**Every reader downloads an engine.** The markdown module is small; the
typst module is thirty megabytes, cached for a year under a digest URL, so
it is paid once per browser and not per document. That is the price of
storing nothing rendered, and it is the right trade for this project; but a
server that has the engine natively could render on request, keep the
result in memory, and store nothing, for a client that cannot run the
module. That is compatible with the third rule and left out of the first
version. See the open questions.

## Comments know their checkpoint

Two fields on `Comment`, both set by the server in `apply`, never by the
client:

- `revision`: the SHA of the checkpoint the comment was made on, which is
  the checkpoint `apply` takes or finds current, by the rule above.
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

This panel replaces manual version saving. A close warning is needed only
while edits have reached neither durable local storage nor the server.
The toolbar reports connectivity and pending server persistence:
"offline, changes kept in this browser" while the socket is down, with
y-indexeddb holding the local state until it is back.

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

**In the reader.** The save button, `dirty`, `savedSource`, `baseSHA`,
`shownSHA`, `published()`, and the 409 handling go. The state badge reports
connectivity and pending durability; a close warning remains when changes
have reached neither durable local storage nor the server. Ctrl-S reports
the actual persistence state rather than claiming pending writes are saved.
`startEditing` no longer
seeds or fetches source: it unfolds the source pane over the Yjs document
the page already holds for reading. `paintPreview` runs for readers too, on
a longer timer. The toolbar while editing is layout, comments, history; while
reading, comments, history.

**In the server.** The room holds the document rather than relaying it.
The server had nothing to do with the text but relay it; now it takes
checkpoints when no browser may be present, applies a restore or a
command-line publish as a diff into the live document, and persists the
state, all of which need the document rather than its updates. The room
keeps a `yrs::Doc` per open document, applies every relayed update to it, and
answers `y-open` from it: `y-state` carries the encoded state as a single
update or an HTTP reference for large state, as specified above, and
`y-snapshot` is never sent, though clients keep answering it for an older
server. `y-open` is answered for any socket that may read the document,
with incoming `y-*` still dropped from anyone below editor, the role
`03-SPEC-sharing.md` defines and which, until that spec lands, the owner alone
holds, from however many tabs, machines and sync clients they like. `end_editing` takes a checkpoint instead of
forgetting. `POST /api/documents` on an existing slug carries
`source` and `source_format` and no `html` or `base_sha`; it becomes an edit
into the session, followed by a checkpoint, and cannot conflict. `PutError::Stale`
and `prune_other_versions` go. `seed_examples` renders the seeded markdown in
memory to anchor the example comments, as `visible_text` does today from
stored HTML, and stores the source.

**In `04-SPEC-sync.md`.** Already revised to this model: the sync client
joins a session the server holds, never seeds, never publishes, and asks for
a checkpoint when it writes the file.

**In the README.** Destroy already promises to delete history. The
description of saving, of "reload before saving", and of the two-tab
conflict comes out, and a paragraph about the timeline goes in.

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

1. **The document on the server.** A `yrs::Doc` in the room; `sessions/<slug>`
   written on a debounce; `y-open` answered from the document for any reader;
   a server restart that comes back with the text intact. Tests: two
   browsers, one restart, no words lost; a reader receives updates and
   cannot send them.
2. **Checkpoints.** The quiet timer, the last-editor rule, the comment
   rule, the `y-checkpoint` message; the manifest and the objects in the
   order above; `size` including history; `--history`, `--checkpoint`;
   `remove` deleting the prefix. Tests: quiet after quiet writes nothing, a
   comment lands on a checkpoint whose text contains its quotation, a crash
   between the index and the manifest is repaired by the next checkpoint.
3. **Reading from source.** The shell at `/raw/<slug>/`, the reader
   rendering the live text and painting the frame, format `html` sent as
   is; the HTML object, `prune_other_versions` and `PutError::Stale`
   removed; `POST` on an existing slug as an edit into the session.
4. **The reader loses its save.** Everything in "What changes elsewhere,
   in the reader". The connectivity indicator and y-indexeddb.
5. **The timeline.** `GET .../history`, the label `PATCH`, `komodoc
   history` and `komodoc label`, the panel, viewing a checkpoint.
6. **Comments know their checkpoint.** The two fields, set in `apply`, in
   both exports. The passage-then-and-now line on the card.
7. **The response export.** `--format response` and `--since`.
8. **What changed since.** The word-level diff, one module for the command
   line and the room, exposed to the browser through WASM; the list beside the comments; anchoring
   hunks by quotation.
9. **Diff and restore.** The merge view in the editor, `komodoc diff`,
   `komodoc restore`, restore as a server-side diff into the document.
10. **Provenance.** The git fields from `publish` and `sync`.

Steps 1 to 4 are built (below). 5 to 10 are not: the timeline, the checkpoint
fields on a comment, the response export, the word-level diff, restore and git
provenance all remain, and each of them reads the manifest steps 1 and 2 now
write. 5 to 7 are a day each and 7 is the one to demonstrate. 8 and 9 are the
browser work and take longer. 10 can go anywhere.

## Open questions

- **The source is public to readers.** Accepted above, and the honest
  price of storing nothing rendered. Who a reader is becomes a question
  `03-SPEC-sharing.md` answers: "anyone who may read the document" in the
  storage table means the reader role there, and a `private` document has
  named readers only. Whether a document may ask for its source to be hidden
  from readers who may see the rendered page, at the cost of the server
  rendering for them, is the same question as the next one.
- **Rendering on the server.** Excluded from this plan. Having the native
  engine in the executable does not put compilation in the request path.
  Rendering and visual diffs run on clients; the server handles permission
  checks, synchronization, persistence, quotas, and retention. LaTeX
  readers use the client-produced PDFs described in `05-SPEC-latex.md`.
- **The quiet interval.** Five minutes is a guess in both directions: long
  enough that a paragraph is a checkpoint and not each sentence of it,
  short enough that a session's worth of work is many points rather than
  one. The last-editor rule bounds the loss in any case.
- **How long a session may stay hot.** The Rust server evicts a room
  nobody has open after its state and checkpoint are durable, then reloads
  it from storage on the next `y-open`. Bound open rooms and outgoing
  queues from the first version; disconnect slow peers with a recoverable
  resynchronization path instead of retaining unlimited queued updates.
- **Documents that need files beside them.** A typst source that `#import`s
  a file or reads a `#bibliography` renders on the author's machine and
  nowhere else, because the engine's file map in the browser has no
  directory. Stored HTML used to hide this from readers; now it does not. The
  answer is the project, several files travelling with the source, which
  remains unbuilt; `04-SPEC-sync.md` is the natural place to feed it. Until then such a document is published as HTML.

## What was built

Steps 1 to 4, on the existing Rust host. No JavaScript host, no server-side
compilation, and no new deployment target: `komodoc/` is still the server and
the command line, `engine/` still renders natively and as WebAssembly, and
`web/` is still the browser application.

### Yrs, and what it turned out to need

Yrs is accepted, at `yrs = "0.27"` against `yjs 13.6.32`. The evidence is
`komodoc/src/tests/yjs.rs`, which does not model a browser: it runs one.
`web/scripts/yjs-peer.mjs` is a line-oriented process holding real `Y.Doc`s
with the same `yjs` and `y-protocols` packages the editor bundles, and the
tests drive it against the same `session.rs` the room uses. What is checked is
the list this spec asked for: a v1 update in each direction, deletions,
state-vector exchange in both directions, two browsers and the server editing
concurrently and converging, awareness relayed between browsers, UTF-16
positions across an astral emoji and a two-byte BMP character, and a restart
that comes back from the persisted state and takes the browser's unsent work
by state vector.

One thing had to change for any of that to hold, and it is the finding worth
carrying: **yrs counts positions in UTF-8 bytes unless told otherwise, and Yjs
counts them in UTF-16 code units.** Left at the default, an update from a
browser naming an index past a non-ASCII character is applied in the wrong
place, and one past an astral character panics inside yrs with a subtraction
overflow. `session::new_doc` builds every document with
`OffsetKind::Utf16`; nothing else about the integration needed changing.

Transactions are opened and dropped inside single synchronous functions in
`session.rs`, so no Yrs transaction is ever held across an await; the room's
mutex is what serializes mutations.

### The document on the server

`komodoc/src/session.rs` holds the CRDT operations, and `RoomState::session`
holds one `yrs::Doc` per open document. `Room::load_session` brings it back
from `sessions/<slug>`, and seeds it from the published source exactly once,
the first time a document that predates this is opened. `y-open` carries the
sender's state vector and is answered from the document for anyone who may
read it; `y-update` is applied before it is relayed, and what is relayed is
what was applied. `y-*` writes from below the editor rung are dropped.

Durability is separated from relaying, which is the point of the whole
protocol. `Room::persist` writes `sessions/<slug>` and only then sends each
socket a `y-ack` naming the highest update of theirs that is now durable. The
sweeper in `serve.rs` runs once a second and writes a room that has been quiet
for `write_after_seconds`, so a document nobody has open is still persisted and
still gets its checkpoints; `Ctrl-C` flushes every open room before the process
goes.

Bounds, all in `Configuration::session`: a bounded per-socket queue (a peer
that cannot keep up is disconnected and resynchronises on reconnect, rather
than being queued for), a ceiling on rooms held in memory with the idle ones
persisted and evicted, a per-socket update rate, and a size ceiling enforced on
the document's text rather than on one update — an update that carries the
source past `max_html` is trimmed back at once and the socket that sent it is
closed with the reason, so no number of concurrent writers can talk their way
past the quota between them.

For a state larger than `inline_state_max`, `y-state` carries a signed,
short-lived, same-origin `ref` instead of an inline update;
`GET /api/documents/<slug>/state` answers it, checks the document's own read
permission rather than trusting the signature alone, and refuses a cross-site
fetch. The browser applies what it fetches and then sends its own state, which
is what catches up anything that arrived during the fetch.

### Checkpoints

`komodoc/src/history.rs` is the manifest; `Room::checkpoint` takes one on
quiet, when the last editor leaves, before a comment is stored, and on a
`y-checkpoint` or a publish. A request from outside within
`CHECKPOINT_DEFER_SECONDS` is deferred and taken when the window passes, so a
burst of saves is one mark. A checkpoint whose SHA is already in the manifest
is not written again and adds no entry.

The write order is the one above — the checkpoint object, the session state,
the index entry, the manifest — and `Room::repair` is the other half of it: the
index names the newest checkpoint, so an entry the manifest has never heard of
whose object is present is a write that stopped after step 3, and the next
checkpoint names it as `parent`. `--checkpoint` and `--history` are flags;
`remove` takes `sessions/` and `history/` with the document.

Quota accounting moved with the storage. An index entry's `size` is the session
state plus every checkpoint, recorded by `Store::record_history`; a checkpoint
is never refused for a quota, and what gives instead is the oldest unlabelled
history, then the oldest labelled, never the newest, against
`Store::room_for`.

### Reading from source

`/raw/<slug>/` on the documents origin serves an empty page with the agent in
it, under the same CSP and the same origin as before; the reader joins the
session, renders the text with the engine, and sends the page in with the
`preview` message the editor already used. The agent installs the styles of
the page it is sent, which it never had to do while the frame arrived as a
rendered document.

"Format `html` sent as it is" turned out to mean served rather than sent. The
`preview` channel sets `innerHTML`, which does not run scripts, and an HTML
document is exactly the kind that carries its own -- a notebook's plots, a
Quarto page's figures. So `/raw/<slug>/` answers an `html` document with the
document, agent injected, as it always did; every other format gets the empty
shell. A reader of an HTML document still sees changes: the frame is served from the
live document, so reloading it is showing the current text, and the reader
reloads it when the text has actually moved and the typing has stopped. The
price is a second of latency and a reload rather than a repaint -- the scripts
run again, and the reader loses their place -- which is the trade against
showing a live page as an inert copy of itself. `make smoke` checks in a real
browser that the scripts run, that an edit reaches a second reader, and that
they still run after the reload. `Publication` lost `html`, `digest`
and `base_sha`; `PutError::Stale` and `prune_other_versions` are gone;
`POST /api/documents` on an existing slug diffs the source into the live
session and takes a checkpoint, so two publishes never conflict. The source
endpoint answers the live document, to anyone who may read it — the change of
policy this spec accepts.

### The reader loses its save

`web/src/lib/collab.js` holds the browser half: it joins with a state vector,
keeps every update until its `y-ack` arrives, keeps the document in
`y-indexeddb` so a reload while the socket is down loses nothing, and sends its
whole state after each join so the work done on either side of a gap reaches
the other. `Reader.svelte` has no save button, no `dirty`, no `savedSource`,
no `baseSHA`, no `shownSHA` and no 409: a reader and an editor join the same
session, a reader renders on a one-second timer and an editor on sixty
milliseconds, the badge says "saving…" or "offline, changes kept in this
browser" rather than "saved", Ctrl-S reports the actual persistence state, and
the close prompt is left only for work that has reached neither this browser
nor the server.

### Migration

Nothing existing is rewritten until a document is first opened, and nothing is
removed until its source is durable as a checkpoint. That is the rollback
boundary, and it is per document and crossed early: before a document's first
checkpoint the previous release reads it unchanged, and after it the old
objects are gone and only this release can read the document. `TODO.md` gives
the boundary and the backup and recovery procedure in full. It is not
rollback-safe once a document has been checkpointed. `Room::load_session` seeds
from `sources/<slug>/<sha>`, from the unversioned `sources/<slug>`, or -- for a
document published as HTML, which never had a stored source -- from the page
itself; the first checkpoint after that writes `history/<slug>/<sha>` and
`sessions/<slug>`, and only then does `Store::drop_derived` remove the old
`documents/` and `sources/` objects. Comments, ownership, `created_at`,
`example`, and `source_format` are untouched throughout, and
`/raw/<slug>/<sha>.html` keeps answering for as long as an old object is there,
so a link written down before this still resolves.

### Verified in a browser

`web/scripts/browser-smoke.mjs`, behind `make smoke`, drives headless Chromium
over the DevTools protocol against a real `komodoc serve` on a temporary
directory. Twelve checks, and they are the ones the protocol tests cannot
make: an editor's browser renders the source into the frame and what is typed
reaches it; a reader renders the document, is given no editor, and sees an
edit arrive without reloading; an HTML document's own scripts run, an edit
reaches a second reader, and the scripts still run afterwards; the toolbar
says "offline" rather than "saved" when the socket is cut, and what was typed
while it was down reaches the server when it comes back; a document that does
not compile says so. Typing goes through real key and text events rather than
through CodeMirror's internals.

### What is not built

Steps 5 to 10. In particular there is no `GET /api/documents/<slug>/history`,
no label, no `komodoc history`, no timeline panel, no restore and no
`revision`/`resolved_in` on a comment; a comment lands on a checkpoint, but
does not yet record which. `02-SPEC-diagnostics.md`'s reader fallback to the
last checkpoint that compiles waits on the same endpoint, and until it exists a
reader joining a document that does not compile is shown the diagnostics page.

The size ceiling is decided before the document is touched, as the spec asks:
`session::admit_update` answers by a bound in the ordinary case -- a v1 update
carries its inserted text inside itself, so the result is at most the current
text plus the update's own length -- and rehearses the update on a scratch copy
only when the bound cannot decide. A refused update is never applied and never
relayed, and the socket that sent it is closed. On a megabyte of prose a
keystroke costs 18 µs to admit and the rehearsal 44 ms, and the rehearsal is
only ever paid on the path where a socket is about to be closed.

Ownership is enforced rather than advisory. `take_room_lease` is a renewable
lease carrying an epoch that rises every time it changes hands, so a server
that stalled long enough to be taken over finds out at its next renewal instead
of writing over the new holder; and a holder stops writing a guard's width
before its lease could be taken, rather than trusting its own clock against
somebody else's. Behind the lease, every object a room owns -- the session, the
manifest, the comments -- is written with compare-and-swap against the version
this server last saw, so a former owner's write is refused by storage itself.
Deployment-wide quota admission is serialised the same way: a `put` whose index
write loses the swap re-reads the index and decides the quota again against the
one that won, so two processes sharing a bucket cannot both spend the last of
it.

What that does not cover is a store with no conditional writes. `--single-writer`
asserts exactly that, and with it asserted the fencing is the operator's
promise rather than the bucket's.
