# SPEC: compiler errors, shown where they are

Status: built, steps 1 to 6. Step 6 landed with `01-SPEC-history.md` steps 1
to 4, which made readers render. See "What was built" at the end. It stands on its own, but it is
written for the editor `01-SPEC-history.md` describes, where readers render the
text themselves; the two places that spec changes what this one does are
marked.

## The problem

Typst is a programming language, and a document written in it stops
compiling several times an hour: an unclosed `$`, a `#let` with no body, a
function given the wrong argument, an import that is not there. Markdown
never fails, so nothing in Komodoc was built for failure, and what a typst
author gets when it happens is the accident of that.

The engine returns `Result<String, String>`: the page, or the message of the
*first* diagnostic, with everything else dropped -- the line, the column,
the hints typst attaches, the second error, and every warning, which the
compiler reports and the engine never looks at. The browser turns that
string into a thrown `Error`, and the toolbar badge shows it in the error
tone, truncated to the width of a badge, next to "saved". The preview keeps
the last page that compiled, which is right, but nothing says so, and
nothing points at the line. An author with a two-hundred-line document reads
"unexpected end of file" and starts scrolling.

The command line is the same string with a prefix: `typst could not compile
paper.typ:` and the message, no location. The `typst` binary itself prints
the file, the line, the column, the offending span underlined, and the
hints; a publisher who has it installed runs it to find out what `komodoc
publish` would not tell them. That is a reason to keep a second compiler
installed, which the port to Rust was meant to end.

Both the Overleaf and typst.app research notes list a parsed error panel
with jump-to-line as table stakes (`market-research/`, both §1 and §2), and
typst.app's free tier explains errors inline. An editor that renders typst
but cannot say where it broke is not a typst editor.

## The decision

Four rules.

**The engine reports diagnostics, not a message.** A compile returns the
page if there is one, and a list of diagnostics either way: errors when
there is no page, warnings when there is. Each carries its severity, its
message, its hints, and where it is -- a file, a line and a column -- when
typst knows. The browser and the command line get the same list, because
they get it from the same crate.

**Errors are shown where they are.** In the editor, an error is an
underline on the span it names, a mark in the gutter of that line, and its
message and hints on hover; the badge counts them, and clicking it goes to
the first. On the command line, an error is printed the way `typst` prints
it: file, line, column, message, hints. Nobody reads a message and then
goes looking for the line.

**The last page that compiled stays up.** A document that does not compile
is an ordinary state of an editor, and the preview is not taken away for
it. What is painted is the last text that compiled; what is said is that it
does not compile now, and where.

**Errors appear slowly and disappear at once.** Half of what typst calls an
error is a construct that is not finished being typed. An underline that
appears at the second character of `$x$` and vanishes at the third is
noise, and an editor that flashes red as you type teaches you to stop
looking at it. So a diagnostic is painted only after the source has been
quiet for longer than the render debounce, and a successful render clears
every diagnostic the moment it lands.

Markdown and HTML never produce a diagnostic (`06-SPEC-html.md`), and nothing
here changes for them: the list is empty and every surface below is inert.

## What a diagnostic is

```json
{
  "severity": "error",
  "message": "unclosed delimiter",
  "hints": ["expected `$`"],
  "file": "",
  "line": 12,
  "column": 7,
  "end_line": 12,
  "end_column": 8
}
```

- `severity` is `error` or `warning`, typst's two.
- `message` is typst's message, unchanged.
- `hints` are typst's hints, followed by its trace -- "error occurred in
  this call of function `f`" -- one entry per line, so a caller sees where
  a failure inside a function was reached from without a second structure
  for it.
- `file` is empty for the document itself, and the path typst asked for
  when the span is in an imported file. In the browser it is always empty:
  a browser compiles with no files beside the document.
- `line` and `column` are one-based, and the span ends at `end_line` and
  `end_column`, exclusive. A diagnostic with no span -- typst reports some
  without one, "no math font found" among them -- has `line` 0 and no
  end. The column counts UTF-16 code units, because that is what the editor
  counts in, and it is computed once in the engine rather than in every
  host: the engine has the line's text and the offset, and the browser has
  neither until it has decoded the document a second time.

The engine keeps `Result<String, String>` nowhere. `typst::compile_html`
and `typst::render` return a `Compiled { page: Option<String>, diagnostics:
Vec<Diagnostic> }`; the page is present exactly when no diagnostic is an
error. `Diagnostic` derives `Serialize`, and serialising it is the engine's
job, so the JSON above is the same bytes on both sides of the ABI.

The span-to-line mapping is typst's own. A diagnostic's `span` is a
`DiagSpan`; `DiagSpan::id()` names the file, `WorldExt::range(span)` on the
world that compiled gives the byte range, or `None` for a detached span,
and the source's `Lines` give `byte_to_line` and `byte_to_column` from
there. Typst's column counts characters; the engine re-counts the line's
head in UTF-16 units, which is the one piece of arithmetic here. The world
built for the compile already holds every `Source` it read, so the mapping
costs nothing new. `typst::compile` returns `Warned<SourceResult<_>>`:
errors are the `Err`, warnings are `.warnings`, and both are the same
`SourceDiagnostic` with its `severity`, `message`, `hints` and `trace`.

### The ABI

`compile` today returns one length, and `output_ptr()` and `ok()` say what
it is. It keeps doing that, and grows one result beside it: after any
`compile`, `diagnostics()` returns the length of the JSON list and
`diagnostics_ptr()` where it starts. `ok()` remains, and remains what it
was: whether the output is a page. The loader in `renderers.js` reads both,
and `render()` resolves to `{ html, diagnostics }` instead of resolving to a
string or throwing. It throws for what it threw for before -- a module that
could not be fetched -- and never for a document that did not compile,
which is not an error of the loader's.

Two results rather than one JSON envelope because the page is a megabyte
and the diagnostics are a hundred bytes, and wrapping the first in JSON to
carry the second is an encode and a decode of the wrong thing on every
keystroke.

## In the editor

**Painting.** CodeMirror's `@codemirror/lint` is the surface: `setDiagnostics`
takes a list of `{from, to, severity, message}` and draws the underline, the
gutter mark and the hover tooltip; `from` and `to` are computed from the
line and column with `doc.line(n).from`. A diagnostic with no span is given
to the panel only, at no position. Hints go under the message in the
tooltip, one per line. Nothing is written for typst that the lint extension
does not already do for every other language.

**Timing.** The preview renders sixty milliseconds after the last
keystroke, as now, and keeps doing so on every render. The diagnostics from
that render are held, and painted when the source has been quiet for four
hundred milliseconds, measured from the same keystroke. A render that
succeeds paints the page and clears the diagnostics immediately, whether or
not the four hundred have passed. So while someone types the source goes
from red to clean at typing speed and from clean to red at reading speed,
which is the asymmetry the fourth rule asks for.

**The badge.** The toolbar badge that says "saved" or "unsaved changes"
today -- and says nothing under `01-SPEC-history.md`, which removes both --
says "1 error", "3 errors", or "2 errors, 1 warning", in the error tone for
errors and the warning tone for warnings alone. It is empty when there is
nothing to say. Clicking it moves the caret to the first diagnostic and
scrolls it into the middle of the view, through the `goTo` the editor
already has for the lock; a second click goes to the next, round the list.
The badge is the whole of the panel for a single-file document with one
error in it, which is the common case.

**The panel.** The lint panel CodeMirror provides, opened with Ctrl-Shift-M
or from the badge's menu, lists every diagnostic with its line and jumps on
click. Closed by default, and closed by Escape. It exists for the document
with nine errors, and is not a fourth pane: it sits at the foot of the
source pane, inside it, and folds with it.

**Warnings.** A compile with warnings has a page, and the page is painted.
The warnings are painted as warnings, in the warning tone, on the same
schedule as errors. Typst warns about things worth a look and nothing that
blocks reading -- an unknown font family falling back, a deprecated call
-- so a warning never keeps a page off the screen.

**The preview when there is no page.** The frame keeps what it last
painted, and the agent is told nothing, so comments stay anchored to the
text that compiled and the lock still resolves against it. The badge is
what says the preview is behind. A user who opens the editor on a document
that does not compile, with nothing painted yet, sees a page in the frame
that says so plainly -- "This document does not compile" -- with the
diagnostics under it, styled like a document rather than like a crash. The
engine produces that page, from the same list, so the command line and a
reader can show the same one.

## For a reader

Today a reader loads stored HTML and cannot meet an error. Under
`01-SPEC-history.md` a reader renders the text themselves, and meets the same
compile failures the author does, a second behind.

A reader keeps the last page that compiled, as the editor does, and is told
nothing: they cannot fix it, the author is seeing the error at that moment,
and a reader shown a red badge for a typo in someone else's editing session
learns to stop reading while the document is being edited. A reader who
joins while the document does not compile has nothing to keep, and is shown
the engine's "does not compile" page with the diagnostics on it. Falling
back to the last checkpoint that compiled is the better answer, and it is
`01-SPEC-history.md`'s to give, since it owns the checkpoints; it is listed
there as a change this spec asks of it.

## On the command line

`komodoc publish paper.typ` compiles, with the engine inside the
executable, before it uploads. When the
compile fails it prints every diagnostic and exits 1, and uploads nothing:

```
paper.typ:12:7: error: unclosed delimiter
  hint: expected `$`
lib.typ:3:1: error: unknown variable: colour
  hint: did you mean `color`?
error: paper.typ did not compile (2 errors)
```

The form is `file:line:column: severity: message`, which is what every
editor since `grep -n` knows how to jump on, and what `typst compile`
prints once its box drawing is stripped. Hints are indented under their
diagnostic. Warnings are printed the same way and do not stop the publish;
the last line says "published ... (1 warning)". A diagnostic without a
span prints without the location prefix.

There is no `--force` to publish a document that does not compile. `publish`
exists to make a document readable, and a document that does not compile is
not one. `komodoc sync` is the tool for keeping a file and a session the
same whatever state the file is in, and it does not compile at all
(`04-SPEC-sync.md`, "What the client does not render"): a broken file syncs,
and the browser tab shows where it broke. This holds under `01-SPEC-history.md`
too, where `publish` on an existing slug is an edit into the live session:
it is an edit the command declines to make when the file does not compile,
since readers would render the failure the moment it landed.

## What it is not

- **Not typst-ide.** Completions, hover documentation and go-to-definition
  remain future editor work. This spec gives them what they
  will need -- a second result channel out of the module, a
  line-and-column convention, the lint extension already mounted -- and
  builds none of them.
- **Not a syntax checker between renders.** The parser could report syntax
  errors in microseconds without compiling. It is not asked to, because the
  compile already runs on every keystroke and already parses; a second
  pass would find the same errors sooner, which the fourth rule says is
  the wrong direction.
- **Not an explanation.** typst.app explains errors in prose. Typst's own
  hints are good and are shown; nothing is generated beyond them.
- **Not a server feature.** The server compiles nothing today and compiles
  nothing here. The list is made in the browser or in the command line,
  costs a few hundred bytes of JSON, and never crosses the network. Nothing
  in this spec touches storage or the sandbox budget.
- **Not markdown, not HTML.** Comrak has no failure mode and the identity
  has none, so the list is empty for both. If a markdown linter is ever
  wanted, it is a different spec.

## What changes elsewhere

- **`01-SPEC-history.md`, the reader.** A reader joining a document that does
  not compile should be shown the latest checkpoint that does, not the
  diagnostics page; the checkpoints are that spec's, so the fallback is
  written there. Until it is, the reader gets the diagnostics page.
- **The Rust engine.** `Result<String, String>` becomes
  `Compiled`; the byte-identical native-versus-wasm test compares the
  diagnostics list as well as the page.
- **`komodoc seed`.** Renders the seeded typst example locally and must
  fail loudly, with the list, if the example ever stops compiling against a
  new pinned typst.

## Steps

1. **The engine.** `Diagnostic` and `Compiled` in `engine/src/typst.rs`,
   built from `Warned<SourceResult<_>>`: errors from the `Err`, warnings from
   `.warnings`, both mapped through the world's sources. Tests: an unclosed
   `$` reports line and column; an error in an imported file names it; a
   deprecation warning comes back beside a page; a spanless error has line
   0; a column past a multi-byte character counts UTF-16 units.
2. **The ABI.** `diagnostics()` and `diagnostics_ptr()` in `abi.rs`; the
   markdown module exports them too and returns `[]`, so the loader has one
   shape. `renderers.render()` resolves to `{ html, diagnostics }`.
3. **The command line.** `render_typst_document` returns `Compiled`;
   `publish` prints the list in the form above and exits 1 on errors, warns
   and proceeds on warnings. Test: a fixture with two errors produces two
   `file:line:column:` lines and no upload.
4. **The editor.** Add `@codemirror/lint`, which the web build does not
   have yet, and mount it; `Editor.svelte` exports
   `setDiagnostics(list)` and `nextDiagnostic()`. `Reader.svelte` holds the
   last list, paints on the four-hundred-millisecond timer, clears on a
   successful render, and counts into the badge. Fixture test: a render
   that fails at 60 ms and succeeds at 200 ms never paints; one that fails
   at 60 ms and is left alone paints at 400 ms.
5. **The page for no page.** `typst::diagnostics_page(list, title)` in the
   engine, on the shared page template; sent to the frame when there is
   nothing else to show.
6. **The reader**, once `01-SPEC-history.md` makes readers render: keep the
   last page, show the diagnostics page on a cold join.

## Open questions

- **Four hundred milliseconds** is a guess at reading speed against typing
  speed. It should be tuned once against a real session, and then not
  exposed as a setting.
- **Squiggle or gutter only.** The underline is the standard, and the
  standard for prose editors is quieter: Overleaf marks the gutter and
  leaves the text alone. Start with both, as the lint extension does, and
  drop the underline if it fights the comment highlights that share the
  text.
- **Warnings in the badge.** A document with one permanent warning -- a
  font it will never have -- shows a yellow badge forever. Typst has no
  suppression; a per-document "hide warnings" is the likely answer and is
  not designed here.

## What was built

Steps 1 to 5, on the existing Rust host and its native/WASM engine.

**The engine.** `engine/src/diagnostic.rs`: `Diagnostic`, `Severity` and
`Compiled`, with the JSON written by hand rather than by serde, so neither
module carries a serialiser for a hundred bytes. `typst::compile_html` and
`typst::render` return `Compiled`; the mapping from `SourceDiagnostic` goes
through `WorldExt::range` and the source's `Lines`, and the column is
re-counted in UTF-16 units in the engine. Hints are typst's hints followed by
its trace. Tests: an unclosed `$` reports its line and column, an error in an
imported file names it, a font warning comes back beside a page, and a column
past an emoji counts UTF-16 units.

**The ABI.** `diagnostics()` and `diagnostics_ptr()` beside `output_ptr()`,
two results rather than one envelope; both modules export them, and the
markdown one always answers `[]`. `renderers.render()` resolves to
`{ html, diagnostics }` and no longer throws for a document that did not
compile -- `html` is null instead -- and still throws for a module that could
not be fetched.

**The command line.** `render_typst_document` returns `Compiled`; `publish`
prints `file:line:column: severity: message` with the hints indented, uploads
nothing when any of them is an error, and counts the warnings into the line it
prints when it does upload. `seed` does the same, loudly, so an example that
stops compiling against a new pinned typst is heard about here.

**The editor.** `@codemirror/lint` is mounted -- gutter, underline, hover, the
panel on Ctrl-Shift-M -- and `Editor.svelte` exports `setDiagnostics`,
`nextDiagnostic` and `showDiagnosticPanel`. `Reader.svelte` holds the last
list, paints it four hundred milliseconds after the source goes quiet, clears
it the moment a render succeeds, and counts it into a badge that goes to the
next diagnostic when clicked. The save that used to be refused for a document
that does not compile is gone with the save itself: under
`01-SPEC-history.md` the source is the document, so there is nothing to refuse
-- a document that does not compile is stored like any other, and what does
not happen is that a reader is shown a page for it.

**The page for no page.** `diagnostic::diagnostics_page(list, title)`, on the
shared page template, reached through a `failure_page(title)` export that
builds it from what the last compile said. The reader sends it to the frame
when a compile fails and nothing has ever been painted, which is what someone
who opens the editor on a document that does not compile sees.

**The reader.** Step 6, once `01-SPEC-history.md` made readers render their own
text. A reader keeps the last page that compiled and is told nothing else: no
badge, no underline, no panel. `paintDiagnostics` returns without doing
anything unless the source pane is open, so a compile failure in somebody
else's editing session never reaches a reader who cannot act on it, while the
last page that compiled stays on the screen. A reader who joins while the
document does not compile has nothing to keep, and is shown the engine's
"does not compile" page with the diagnostics on it, which is the same
`failure_page` the editor shows on a cold open.

What is not built: the better answer for that cold join, which is to fall back
to the latest checkpoint that compiles. `01-SPEC-history.md` owns the
checkpoints and its step 5 is what would expose them to the browser; until
then the diagnostics page is what a cold join gets, as this spec said it would
be. Two details also differ from what is written above: the four hundred
milliseconds are measured from the render finishing rather than from the
keystroke, so the wait is that much longer than the number says; and there is
no fixture test of the timing, only of the engine, the command line and the
editor's placement of a span.
