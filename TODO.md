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
  `Catalog::documents_page` already exists as the keyset primitive and has
  no callers yet.
- A room fenced as `FenceReason::Oversized` stays read-only until its
  instance is evicted, and eviction refuses a dirty session, so an oversized
  write leaves no way back short of a restart.
- Comment, reply and suggestion catalogue writes do not carry the session
  generation that ownership transfer and room-level mutations check.

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

- The sandbox's `max_assets` is the default 32 MB. Nobody has measured what
  a public deployment on one disk can afford, so the number is still a
  default rather than a decision.
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

- The LaTeX browser acceptance scripts (`web/tools/latex-e2e.mjs` and
  `web/checks/latex-browser.mjs`) run Chromium. Other workflows already have
  Firefox checks, but LaTeX still needs Firefox and Safari coverage and
  validation on memory-constrained devices.

## Quarto

Source preview through the paired local app, capture, binding, sync, and
isolation all shipped. What did not:

- The draft passes `:::` divs through verbatim. Callouts, columns, tabsets,
  margin content, conditional content (`when-format`, profiles), and
  shortcodes other than `{{< include >}}` render as their source. The
  parser in `web/src/lib/engines/quarto.js` already tracks fences and
  spans; the missing part is a rendering for each family.
- The draft applies none of `echo`, `include`, `output`, `eval` or
  `code-fold`: every cell shows verbatim. That was chosen over guessing.
- Quarto preview is HTML only. `QuartoWatch::kind()` in
  `crates/librepaper/src/local/preview/quarto.rs` is hard-wired to HTML;
  a `--to pdf` preview would use the existing PDF reader, as Calepin's does.
- Website and book projects are collected and tested on the capture side
  but the live preview accepts document scope only.
- `librepaper local doctor` reports Quarto but never the Calepin tool.
- No declared Quarto version support matrix, and the real-toolchain R and
  Python tests are all `#[ignore]`d with no job that runs them. The
  end-to-end gate (pair, live repaint, disconnect, reconnect, R then Python)
  has no automated run.
- `web/tools/quarto-benchmark.mjs` prints draft timings but asserts no
  budget and has no large fixture.
- Confinement (`bwrap`, `sandbox-exec`, none) is detected and reported, but
  there is no model for granting a render access to data or the network.

## Dictation

- The dictation section of the settings panel is inside the reader's
  settings tab, which only editors can open. Readers who only comment cannot
  change the model.
- The local app could serve model files: one download shared by every
  browser on a machine and a pinned copy on disk. A new job kind and a CLI
  command for a small gain, and it excludes everyone without the app. Only
  worth revisiting together with the next item.
- Native inference in the local app. Several times faster than wasm on
  machines without WebGPU, and the only path to Parakeet TDT, Canary, and
  streaming models. It puts an inference engine into the single static
  binary on three platforms, must be gated so the public server never runs
  it, and needs a streaming addition to the local protocol. The capture and
  insertion code in `web/src/lib/dictation/` is what such a backend would
  reuse.
- Serving model files from the LibrePaper server, for self-hosters who want
  no dependency on Hugging Face. Not for the sandbox, whose hosting limits
  large files. A small addition when wanted.
- A streaming model as the default. Words while speaking is a better feeling
  than words after a pause, but the models that do it in the browser today
  are English-centric or weaker than Whisper small. Revisit when the catalog
  can take one without a second code path.
