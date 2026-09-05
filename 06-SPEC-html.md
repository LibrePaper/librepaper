# SPEC: HTML, a source format like the other two

Status: built on the Rust host, without
waiting for `01-SPEC-history.md`; see "What was built" at the end. It is a small
spec with one decision, and most of its length is the list of places that
decision reaches.

## The problem

Komodoc has three formats and two of them are documents you can edit. A
markdown or typst file keeps its source beside the page it rendered to, and
opens in the editor: the source on one side, the document on the other, the
comments beside both. An HTML file keeps nothing. The server drops a source
in any format it does not list, and it lists two (`config.rs`,
`source_formats`); `publish` says an HTML document "is its own source and
keeps none"; the source endpoint answers "publish it again from its
markdown to edit it"; the landing page marks which documents are editable
and which are not; the README has a paragraph explaining the difference;
and `01-SPEC-history.md`, which otherwise makes every document a live session,
keeps HTML as the one format "sent as it is", and leans on that to offer a
place for a source that must stay private.

The one format that cannot be edited is the one most documents arrive in.
Quarto, Jupyter, marimo and every notebook on the examples page publish
HTML, because HTML is the only thing they all produce. So the editor, the
live preview, the co-editing, the checkpoints and `komodoc sync` reach the
minority of documents that were written in markdown or typst by hand, and
the author of a Quarto paper who wants to fix a sentence between renders
downloads nothing, edits nothing, and re-renders the whole thing.

There is no reason for the difference. "Its own source" is a description
of a renderer, not of a document: the renderer that takes HTML to HTML is
the identity. Once that is said, everything built for the other two formats
applies without modification, and the special case is what has to be
removed.

## The decision

**HTML is a source format, and its renderer is the identity.** A document
whose format is `html` has its HTML for a source, exactly as a markdown
document has its markdown. Rendering it produces the source, byte for byte,
with no page template around it: an HTML document is shown as it was
published, which it always was, and now for the same reason a markdown
document is shown as its markdown says.

Nothing else is decided, because nothing else is different. The source is
edited in the same editor, previewed in the same frame, shared through the
same Yjs session, checkpointed on the same schedule, re-anchored by the
same agent, mirrored by the same sync client and published by the same
command. Every sentence below is that one sentence applied to a place in
the code that currently says otherwise.

## What it reaches

**The renderer.** A third renderer, `html`, always on and behind no
feature. The native implementation lives in `engine/src/html.rs`; the
browser short-circuits HTML in `web/src/lib/renderers.js` without fetching
a compiler. `render(source, title)`
returns `source`, and `title_of(source)` returns the `<title>` or, failing
that, the first `<h1>`, which is how the landing page already names an
uploaded HTML file and how the command line does not -- `publish paper.html`
names the document after the filename today, and after this the two agree,
with native title extraction tested against the browser's DOM parser. `is_html` says what `is_markdown` and
`is_typst` say. There is no wasm module for it: the browser's
`renderers.render(source, title, "html")` resolves to the source without a
fetch, and `available("html")` is true in every build, so HTML is the one
format that every deployment can edit whatever it was built with. The Rust
engine gains nothing.

**The server.** `source_formats` gains `html`; a publish of an HTML file
stores its source under format `html`, and the source endpoint serves it.
The "publish it again from its markdown" answer goes, because there is no
longer a document without a source. Under `01-SPEC-history.md` the distinction
has already collapsed -- the source is the document, and `html` is a value
of `format` -- and this spec adds only that the session for it is
writable, as the session for the other two is.

**Documents already published.** Every HTML document on a deployment
already has its source: the HTML that is stored. No republish is needed.
An index entry with a stored page and an empty `source_format` is read as
`html`, lazily, and written back the first time it is touched; under
`01-SPEC-history.md` it seeds the session the same way a markdown source does.

**The editor.** The same `Editor.svelte`, coloured by
`@codemirror/lang-html`, which the web build does not yet have. The same
sixty-millisecond render, which for HTML is a copy. The same `preview`
message to the frame, and the same agent re-anchoring every comment
against what was just typed. The same lock between the caret and the
document, with one adjustment inside it: the lock finds a place by taking
the words around it and blanking the markup characters, and for HTML the
markup is not a character but a tag. `<em>word</em>` flattens to `em word
em` today, and `&amp;` stays as it is. The flattening in `sync.js` learns
to blank a tag and decode an entity when the source is HTML, and nothing
else about the lock changes. This is the one piece of shared code that
has to know the format, and it is a regular expression.

**The reader.** Unchanged. Under `01-SPEC-history.md` a reader renders the
text themselves; for HTML that is the identity, which is what "sent as it
is" already meant.

**Sharing, checkpoints, restore, labels, sync.** Unchanged, and the sync
client gets its best case: `komodoc sync c9k paper.html` beside a Makefile
that runs `quarto render` turns every render into a checkpoint, with no
step between the author's tools and the readers. `04-SPEC-sync.md` needs an
`.html` example and nothing else.

**Diagnostics.** HTML never fails to render. The list is empty, as it is
for markdown (`02-SPEC-diagnostics.md`), and the surfaces are inert.

**The landing page and the README.** The editable-or-not marker goes,
because everything is editable; the extension shown on each row stays,
since it still says what a document was written in. The README's paragraph
on HTML documents that cannot be opened is deleted, and the table of
formats gains a row: HTML, published with `komodoc publish paper.html`,
rendered by nothing, nothing over the wire.

## What is different, and is accepted

Two things about HTML are not true of the other two formats. Neither
changes the decision; both are said so nobody is surprised.

**A generated source is replaced by the next generation.** A markdown
source is the author's text. A Quarto document's HTML is the output of a
`.qmd` the author keeps, and the next `quarto render` produces a new HTML
that contains none of what was typed into the old one in the browser. That
is fine, and it is what `01-SPEC-history.md` is for: the render is a
checkpoint, the browser edit before it is a checkpoint, and nothing is
lost, but the author should know that the `.qmd` is where a lasting change
belongs, and the browser is for the fix that cannot wait for a render. The
editor says nothing about this; it is the nature of a generated file, not a
property of Komodoc.

**A source can be big and its lines can be long.** An HTML file with
resources embedded carries its figures as base64 on single lines of a
megabyte or more. CodeMirror is built for large documents, and Yjs relays a
few dozen bytes per keystroke whatever the document's size, so the
session is fine once joined, and joining fetches the state over HTTP, as
`01-SPEC-history.md` requires for state above the socket's message cap; what suffers is the source pane, where a line-wrapped
megabyte of base64 is slow to lay out and impossible to read past. This is
the one place where treating HTML like the others costs something, and it
is left to the open questions rather than solved here.

**A document runs scripts, and the preview runs them again.** A notebook
with a live kernel or a page full of interactive figures re-executes on
every paint. Readers already pay this once; an editor pays it on every
pause in typing. The preview pane already folds away, which is the answer
for a document whose scripts are heavy.

## What it is not

- **Not a visual editor.** Source only, as for markdown and typst.
  Overleaf's visual mode is `market-research/` §1 and not this spec.
- **Not a sanitiser.** An HTML document already runs on its own origin in
  a sandboxed frame, and a publisher could always upload a script. An
  editor is a publisher, or -- once there is a spec for sharing -- someone
  the publisher chose to trust as one. Nothing new is admitted.
- **Not a converter.** Typst.app converts Word and Markdown to typst on
  import. Komodoc does not convert HTML to anything; it edits it.
- **Not a private source.** `01-SPEC-history.md` offers "publish as HTML" to
  an author whose typst source must not be seen. That offer is withdrawn
  by this spec and was never real: an HTML document's source is the bytes
  every reader downloads. A typst source that must stay private is not
  published to Komodoc as typst, and the HTML rendered from it is exactly
  as private as it was, which is not at all.

## What changes elsewhere

- **`01-SPEC-history.md`.** "Format `html` sent as it is" stays true and
  stops being an exception; it is the identity renderer. The sentence
  making HTML the home of a private source is struck, for the reason
  above. The note that a typst document needing files beside it "is
  published as HTML until the project exists" stands, and that document is
  now editable as HTML, which is more than it was.
- **`04-SPEC-sync.md`.** One `.html` example, and the observation that
  `quarto render` under `sync` is a publish.
- **`02-SPEC-diagnostics.md`.** "Markdown never produces a diagnostic"
  becomes "markdown and HTML".
- **The Rust host and engine.** The renderer list includes HTML as an
  identity renderer, with no separate HTML WASM download.

## Steps

1. **The renderer.** `engine/src/html.rs`: `render`, `title_of`, `is_html`.
   `title_of` scans for `<title>` and `<h1>` without an HTML parser, the
   way the markdown title scan works without a markdown parser; a test pins
   it to what the landing page's `DOMParser` finds on the seeded examples.
   `renderers.js` short-circuits `html`.
2. **The server.** `source_formats` gains `html`; the source endpoint's
   "no stored source" branch and its message go; the lazy `html` default
   for an entry with a page and no format. Test: a document published as
   HTML before this change opens in the editor after it.
3. **The command line.** `publish` stores an HTML file's source under
   `html` and titles it from the engine. Test: `publish paper.html` with
   no `--title` yields the `<title>`, not the filename.
4. **The web.** Add `@codemirror/lang-html`; `Editor.svelte`'s `language()`
   gains a third case; `Reader.svelte`'s edit gate lets `html` through
   with no module; `Landing.svelte` drops the editable marker and its
   tooltip; the README loses its paragraph and gains its row. The lock's
   flattening in `sync.js` blanks tags and decodes entities for HTML. Test:
   a click on an emphasised word in a seeded HTML example lands on that
   word in the source, and an `&amp;` in the source does not stop a jump.
5. **The specs.** The four edits listed above.

## Open questions

- **Long lines.** The honest options are three: do nothing, and let a
  megabyte line be slow; turn off line wrapping for HTML, so the line is
  fast and scrolls sideways for a kilometre; or replace every `data:` URI
  longer than some threshold with an inert chip that says what it is and
  how big, editable around and not inside, as a CodeMirror replacing
  decoration over an unchanged source. The third is a day's work and
  presentation only -- the source, the session and the checkpoints never
  see it -- and it is the recommended answer. It is not in the first
  version, because the first version is the decision and not the
  furniture.
- **The render debounce for a scripted document.** Sixty milliseconds is
  right for a renderer that takes single-digit milliseconds and a page
  that runs nothing. A notebook that takes a second to boot on every paint
  may want a longer pause, or a paint only on a pause in typing rather
  than after every one. Whether this is a per-format constant or something
  measured from the previous paint is not decided.

## What was built

All five steps, on the Rust host. The identity renderer lives in
`engine/src/html.rs` and is reached in the browser by a short-circuit in
`renderers.js` that fetches nothing. `01-SPEC-history.md` is not built, so
what changed is the source endpoint rather than a durable session.

**The renderer.** `html::render` is the identity, `html::title_of` scans for
`<title>` and then `<h1>` without an HTML parser and decodes the entities a
title is likely to carry, `html::is_html` says what its two neighbours say.
`renderers.available("html")` is true in every build, and `renderers.render`
and `titleOf` short-circuit it.

**The server.** `source_formats` gains `html`; a publish of an HTML file, from
the command line or dropped on the page, is titled from its own `<title>` and
is its own source -- stored once, not twice, since the page is the source and
writing a second copy would double what every notebook costs its owner's quota
and the deployment's bill; the source endpoint's "publish it again
from its markdown" branch is gone, and an entry with a page and no format is
answered as `html` from the stored page, so a document published before this
change opens in the editor without being republished. `renderers()` lists
`html` always.

**The editor.** `@codemirror/lang-html` colours it, the edit gate lets it
through with no module, and the lock's flattening in `sync.js` blanks tags and
decodes entities in place -- an entity keeps its decoded character followed by
spaces, so an offset into the flattened copy is still an offset into the
source -- and then applies the same rule every other format gets, since the
rendered side it is matched against always does. Two things about an HTML
source made the lock miss: a newline in it is a break between tags rather than
between paragraphs, and most of the characters in a window are markup, so for
HTML the window is six times as wide and is not stopped at the end of a line.
`scripts/check-sync.js` measures a Quarto example alongside the markdown and
typst ones, over the carets in its text nodes, and finds every one of them.

The flattening also had to learn to say where it had been. Blanking a tag
preserves length, but collapsing a run of whitespace does not, and in an HTML
source most of the runs are blanked tags -- so a match found in the flattened
copy was a position in a text nobody has, and the caret landed near the top of
the file. The copy now carries, for each of its characters, the offset it came
from, and the check asks not only that a place was found but that the word it
lands on is the word the document has there.

**The landing page and the README.** The editable-or-not marker is gone, the
extension stays, the README's paragraph is replaced with what is true now and
the table of formats gains its row.

Not built: the open questions, both of them. A megabyte-long `data:` URI is
still a megabyte-long line, and the render debounce is still sixty
milliseconds for every format.
