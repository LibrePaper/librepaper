# Left open

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Room and storage

- Type `CatalogError::Conflict` so `CatalogError::refusal` stops classifying
  the catalogue's conflict prose by substring. One pinned function, about a
  hundred construction sites.
- Give `storage/backup.rs` a `Catalog` snapshot and verify API so it stops
  opening its own SQLite connection outside the execution boundary and
  shutdown.
- Paginate `catalog_entries` and `documents()`, which are unbounded reads.
- Distributed backup ownership: conditional claims, fencing, stale-owner
  recovery. `BackupOwnership` supports single-authority deployments only.
- A room fenced by the encoded-size backstop stays read-only until it is
  reopened; a transient fault there needs a reopen path.
- Checkpoint rate limiting counts in fixed hourly buckets (`bucket = now /
  3600`) where the doc comments describe a rolling hour.
- Comment, reply and suggestion catalogue writes do not carry the session
  generation that ownership transfer and room-level mutations check.
- Hosted recovery, R2 fencing and automatic takeover are designed in
  [docs/specs/failover.md](docs/specs/failover.md) and not built.

## Known ways work can be lost, kept deliberately

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

## HTML as a source

- A megabyte-long `data:` URI in an HTML source is a megabyte-long line in
  CodeMirror, and slow. The answer is presentation only -- a replacing
  decoration that shows an inert chip saying what the URI is and how big,
  editable around and not inside -- and it is a day's work nobody has asked
  for yet. The renderer is `crates/librepaper/src/document/html.rs`.

## Directories

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

## Signing in

- `Grant.login` in `document/store.rs` holds a handle, not a login, since
  providers arrived; rename the field to reflect provider-neutral handles.

## Sync

- Cursor positions: a sync client could receive them from an editor over a
  local socket and publish them through awareness. Deferred; the presence
  entry does not require an editor plugin or LSP integration.

## LaTeX

- See the "Remaining" list in [docs/specs/latex.md](docs/specs/latex.md).
