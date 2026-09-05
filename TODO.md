# Known ways work can be lost

Left alone deliberately, from reviews of the diagnostics, HTML and history
work:

- The four hundred milliseconds before a diagnostic is painted are measured
  from the render finishing rather than from the keystroke, so the wait is
  longer than `02-SPEC-diagnostics.md` says. There is no test of the timing.
- A caret in a heavily tagged region of an HTML source -- syntax-highlighted
  code, where every token is its own `<span>` -- still finds the document
  about a fifth of the time less often than prose does. The window for HTML is
  already six times the width; the rest is the nature of the markup.
- The document size ceiling is enforced by applying an update and trimming the
  overflow back, not by refusing the update before it is applied. The room's
  text never stays over the ceiling and the socket that did it is closed, but
  for the moment between the two the document is over its limit, and the trim
  is relayed to everyone as an edit nobody made. Deciding beforehand means
  measuring every update against a clone of the document, on every keystroke.
- Two servers on one bucket still coordinate through the advisory room lock:
  the second one finds out it is second and goes read-only. `01-SPEC-history.md`
  asks for a fenced writer lease, and this is not one. A lock that goes stale
  while its holder is alive would let both write a document's session.
- A document is migrated out of the old layout the first time it is opened,
  and its old `documents/` and `sources/` objects are removed at its first
  checkpoint. A deployment rolled back after that point would find those
  documents unreadable, because the rolled-back code cannot read a checkpoint.
- A reader looking at a document whose format is `html` sees an edit on their
  next load rather than as it is typed. The frame is served the page itself,
  so the scripts a notebook or a Quarto page carries actually run; sending it
  over the `preview` channel instead would set `innerHTML`, which runs
  nothing. Markdown and typst readers do see edits live.
