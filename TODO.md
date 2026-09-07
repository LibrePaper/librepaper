# Known ways work can be lost

Left alone deliberately, from reviews of the diagnostics, HTML and history
work:

- A caret in a heavily tagged region of an HTML source -- syntax-highlighted
  code, where every token is its own `<span>` -- still finds the document
  about a fifth of the time less often than prose does. The window for HTML is
  already six times the width; the rest is the nature of the markup.
- A reader looking at a document whose format is `html` sees an edit when the
  frame reloads, about a second after the typing stops, rather than as each
  word lands. The frame is served the page itself so that the scripts a
  notebook or a Quarto page carries actually run; sending it over the
  `preview` channel instead would set `innerHTML`, which runs nothing. Markdown
  and typst readers see each render.
- Admitting an update that would carry a document past its size ceiling costs
  one encode and one decode of the whole document -- 44 ms on a megabyte. It is
  bought only on the path where the update is about to be refused and the
  socket closed, and an ordinary keystroke costs 18 µs on the same document
  (`admission_costs_a_comparison_on_a_large_document` prints both).
- A room's lease is renewed on the write path rather than on a timer, so a
  document nobody is writing holds a lease that goes stale. That is deliberate
  -- an idle room needs no lease -- but it means the first write after a long
  idle spell pays a storage round trip before it lands.

# The migration, and how to come back from it

There is a point of no return, and it is worth knowing where it is.

**Before a document's first checkpoint**, nothing has been removed. The
document is seeded into a session from the objects the old layout wrote, and
those objects are still there: a deployment rolled back to the previous
release reads them and carries on as though nothing happened. What is lost by
rolling back is only what was typed since the upgrade.

**After a document's first checkpoint**, its `documents/<slug>/<sha>.html` and
`sources/<slug>/...` objects are gone, and the document lives in
`sessions/<slug>` and `history/<slug>/`, neither of which the previous release
can read. A rollback past that point makes that document unreadable. The
checkpoint happens the first time the document is opened and then quiet for
`--checkpoint` minutes, when its last editor leaves, when somebody comments on
it, or when it is published to -- in practice, within minutes of the first
person opening it.

So the boundary is per document, not per deployment, and it is crossed early.
Treat the upgrade as one-way once anybody has opened a document.

## Before upgrading

Copy the storage. It is a directory or a bucket prefix, and every object under
it belongs to komodoc:

```sh
# a directory
cp -a komodoc-data komodoc-data.before-history

# a bucket
aws s3 sync s3://your-bucket/komodoc s3://your-bucket/komodoc.before-history
```

Nothing else is needed: `index.json`, `documents/`, `sources/`, `rooms/` and
`examples/` are the whole of what the previous release keeps, and
`session.key` alongside them is what keeps everyone signed in.

## Coming back

To roll back before anybody has opened a document, deploy the previous release
against the same storage. The `sessions/` and `history/` objects it does not
understand are ignored, and it reads what it always read.

To roll back after documents have been opened, restore the copy:

```sh
# a directory
mv komodoc-data komodoc-data.after-history
mv komodoc-data.before-history komodoc-data

# a bucket
aws s3 rm s3://your-bucket/komodoc --recursive
aws s3 sync s3://your-bucket/komodoc.before-history s3://your-bucket/komodoc
```

That returns every document to what it said at the moment of the copy. What is
lost is everything written since, which is why the copy is worth taking
immediately before the upgrade rather than the night before.

## Recovering one document without rolling back

A document's history is plain text under its own prefix, so a single document
can be read out of the new layout without the old release:

```sh
cat komodoc-data/history/<slug>/index.json          # the checkpoints, oldest first
cat komodoc-data/history/<slug>/<sha>               # the source at that checkpoint
```

Each checkpoint object is the document's source as it stood, byte for byte, so
recovering one is a copy. `sessions/<slug>` is a Yjs update and is not meant to
be read by hand; the checkpoints are.

# Still outstanding

- Two servers on one bucket coordinate through a renewable, fenced writer
  lease, and every object a room owns is written with compare-and-swap, so a
  former owner's write is refused by storage rather than by politeness. What is
  not covered: a store with no conditional writes at all. `--single-writer`
  asserts exactly that, and with it asserted the fencing is the operator's
  promise rather than the bucket's.

# HTML as a source, left open

HTML is a source format like markdown and typst: the identity renderer in
`crates/engine/src/html.rs`, its own row in the formats table, the caret lock's
flattening in `sync.js`. Two questions from that work were left unanswered:

- A megabyte-long `data:` URI in an HTML source is a megabyte-long line in
  CodeMirror, and slow. The answer is presentation only -- a replacing
  decoration that shows an inert chip saying what the URI is and how big,
  editable around and not inside -- and it is a day's work nobody has asked
  for yet.
- The render debounce is sixty milliseconds for every format. A notebook that
  takes a second to boot on every paint may want a longer pause, or a paint
  only on a pause in typing; whether that is a per-format constant or
  something measured from the previous paint is not decided.

# Directories, left open

A document is a directory: texts in the session keyed by id, assets by
digest, a checkpoint as a tree, `publish <directory>`, a zip out. What was
deliberately not done:

- The sandbox's `max_assets` is the default 32 MB; it was to be measured
  against the R2 bill and set lower. Cost bounding comes before every other
  concern there, and the number is still a default.
- Fonts for typst. A `.otf` or `.ttf` in the directory is stored and
  versioned and not offered to typst, whose font book is built once from
  the static faces. Building it per compile from those plus the directory's
  is a step of its own, once someone needs a font the engine does not ship.
- A zip in. The editor hands back the directory as a zip; a zip dropped on
  the landing page -- the Overleaf habit -- would make `publish <directory>`
  reachable from the browser. Small, once the routes exist; not scheduled.

# Signing in, left open

- The table of pending terminal sign-ins is capped at a thousand in total.
  One caller can fill it and keep every new `komodoc login` waiting for ten
  minutes; nothing else. A per-address cap would be the better bound.
- `Grant.login` holds a handle, not a login, since providers arrived; rename
  the field to reflect provider-neutral handles.
