# SPEC: bibliography, citations and Zotero

A `.bib` is already an ordinary file in a Komodoc document: it is in
`text_extensions` (`config.rs:383`), it travels with a published directory, it
edits and versions and diffs like any other source. What it is not, yet, is
*known*. A LaTeX document's bibliography is compiled by BibTeX or Biber and
nothing in Komodoc reads it; a Typst document's `#bibliography` is read by the
Typst compiler and nothing else sees it; a Markdown document has no citations
at all. Nowhere can an author type `@` and be offered the paper they mean.

This page specifies one parsed library per document, shared by every format,
and the three things it makes possible: completion while writing, citations
rendered in Markdown, and a bibliography that comes from Zotero without
Komodoc holding anyone's credentials.

## Decision

The library is parsed in `crates/engine`, in a `bib` module beside
`markdown.rs` and `typst.rs`, and exposed through `abi.rs`. One parser serves
the browser's completion, the Markdown renderer's citations, the CLI's
diagnostics and the reader's reference list, because a key that resolves in
one place and not another is worse than a key that resolves nowhere.

Citation rendering is a third engine feature, `citations`, built as its own
WebAssembly module. `markdown.wasm` stays near its few hundred kilobytes and a
document with no bibliography fetches nothing extra; a document that cites
fetches the citation module the way a Typst document fetches `typst.wasm`.
This settles the open question in [quarto.md](quarto.md): citations are not
behind a build flag that some deployments lack, they are behind a fetch that
only citing documents pay for.

Zotero is reached from the author's machine, never from the server. Komodoc
stores no third-party credential and runs no background sync on anyone's
behalf.

## The library

The library of a document is the union of the `.bib` files in its tree, parsed
in path order, with a later file's duplicate key losing to an earlier one and
the collision reported as a diagnostic. A document names its bibliography the
way its format already does -- `bibliography:` in Markdown or Quarto front
matter, `#bibliography("refs.bib")` in Typst, `\addbibresource` or
`\bibliography` in LaTeX -- and when nothing names one, every `.bib` in the
tree is the library. Naming one narrows it; naming none is not an error.

An entry is parsed to `{key, type, authors, year, title, container, doi, url}`
and the raw field map behind it. The parser accepts BibTeX and BibLaTeX field
names, `@string` macros, cross-references and braced-title casing, and it does
not fail the document on a malformed entry: a bad entry is skipped, reported
as a diagnostic at its line, and the rest of the file still parses. An author
mid-paste should get completion on the entries that are already valid.

The browser caches parsed libraries against bibliography contents and resource
configuration, excluding ordinary prose. Changes are debounced, the cache is
bounded, and stale asynchronous results cannot replace the active library.

## Completion

Typing the format's citation opener in the editor opens a completion list over
the library: `@` in Markdown and Quarto, `@` in Typst -- where it is the
reference syntax already -- and `\cite{`, `\citep{`, `\textcite{` and their
relatives in LaTeX. One completion source, three triggers, in
`web/src/lib/` beside the editor.

The list matches on key, author surname, title words and year, in that
priority, and shows the entry the way a person recognises it: author, year,
title, container. Choosing one inserts the key in the syntax the trigger
started, and nothing else -- no bracket balancing surprises, no reformatting
of what was already typed. With no bibliography in the tree, the trigger types
a literal `@` and no list appears.

Completion needs the library and not the citation renderer, so it works in
every format including the ones Komodoc never renders itself. A LaTeX author
gets completion from the same list, compiled by the same BibTeX or Biber the
[wasmtex.md](wasmtex.md) routing already chooses.

## Citations in Markdown

Markdown and Quarto gain the Pandoc citation syntax, which is what the
documents that need it already use: `[@key]`, `@key`, `[-@key]`, several keys
separated by semicolons, and prefixes, locators and suffixes inside the
brackets (`[see @key, pp. 33-35; also @other]`). A key with no entry renders
as the key in brackets and produces a diagnostic; it does not fail the render.

Rendered citations and the reference list are produced by
[Hayagriva](https://github.com/typst/hayagriva), the library Typst formats its
own bibliographies with, so both use the same formatting library. Output also depends on the selected
style, locale, and citation context. The style is named in
front matter (`csl:` for a Quarto document, `bibliography-style:` for plain
Markdown); the shipped set is Hayagriva's built-in styles, and an arbitrary
CSL file in the tree is out of scope until someone asks for one.

The reference list is rendered where a `# References` heading appears, and at
the end of the document when none does -- the rule Quarto keeps.

Rendering injects text the source does not contain: `(Smith 2020)` where the
source says `[@smith2020]`, and a whole reference list at the end. This is the
same hazard [quarto.md](quarto.md) raises for callout titles and figure
numbers, and the same answer applies: a comment must anchor to the words
around the citation and not to the injected ones. Confirm it against the
source-anchoring path before this ships, with a test that comments on a
sentence containing a citation and survives a change of style.

## Diagnostics

The library contributes to the existing diagnostics: a cited key with no
entry, a duplicate key across files, a malformed entry, and a `.bib` named in
front matter that is not in the tree. Each carries a path, a line and the
offending text, so it reaches the Diagnostics panel and, through
`komodoc agent diagnostics`, an agent that can
explain it.

An entry present but uncited is not a diagnostic. Authors keep a library
larger than one paper on purpose.

## Zotero

Zotero holds the library the author actually maintains, and the useful
integration is the one that puts it in the tree as a `.bib` without a
credential ever reaching Komodoc.

**From the terminal.** `komodoc bib pull` reads the author's Zotero library on
their machine and writes it into the document's tree through the existing
publish path:

```sh
komodoc bib pull "$KOMODOC_DOCUMENT" --collection "Paper: sandbox costs"
komodoc bib pull "$KOMODOC_DOCUMENT" --group 2451 --into references.bib
```

The default target is `references.bib`, replaced whole on each pull. Entries
are written in a deterministic order with stable keys and stable field
ordering, so a pull that adds one paper is a one-entry diff in the history and
not a rewrite of the file. The pulled file belongs to the pull: hand edits to
it are overwritten, and an author who wants entries of their own keeps them in
a second `.bib`, which the library unions in. Saying that plainly is kinder
than a merge that guesses.

**From the local app.** The local app in `crates/komodoc/src/local/` already
discovers tools on the author's machine for LaTeX; it gains one more
discovery, a Zotero running on its loopback port, and exposes search and
export over the existing bridge protocol. With it, the browser's completion
list can offer entries the tree does not have yet, and choosing one adds the
entry to the pulled `.bib` and inserts the key in one gesture. Without it,
completion is over the tree, which is the whole feature minus the convenience.

The exact loopback endpoint, its version negotiation and what Zotero versions
expose it belong to the implementation milestone, in the manner
[wasmtex.md](wasmtex.md) defers local transport research. The capability is
reported like any other local capability: present, absent, or incompatible,
never assumed.

**What is deliberately not built.** Not a server-side Zotero sync. It would be
the first third-party credential Komodoc stores and the first background job
it runs for a user, and it would drag in secret storage and rotation, token
revocation, the interaction with the retention and erasure rules in
[catalog.md](catalog.md), and a rate-limited third-party API on the serving
path. A browser-only author with no Zotero on the machine exports a `.bib`
from Zotero and uploads it, which is the workflow they have today and which
keeps working.

## What is not done here

- No reference manager other than Zotero. Mendeley and ReadCube can reuse the
  pull command's shape if anyone asks; nothing here is Zotero-specific except
  the discovery.
- No citation rendering for LaTeX. Its bibliography is compiled by BibTeX or
  Biber through the existing routing, and this spec does not touch it beyond
  giving its author completion.
- No CSL file in the tree, no citation style editor, no per-collaborator style.
- No searching external databases -- Crossref, PubMed, arXiv -- from the
  editor. That is a different feature with a different failure mode.
- No writing back to Zotero. The pull is one-way.

## Delivery scope

The current implementation covers the shared library, source completion,
Markdown citation rendering, and browser/agent diagnostics (steps 1–3).
Completion uses `bibliography.wasm`; Markdown formatting uses `citations.wasm`.
Both ship with the server at content-addressed URLs and are built by `make wasm`.
The Zotero pull command, local discovery, and add-from-search gesture (step 4)
remain future work. Quarto citation syntax is supported by the shared helpers;
registering and rendering `.qmd` documents belongs to the Quarto spec.

## Order of work

1. The `bib` module in the engine: the parser, the library, the resource
   cache, and the diagnostics. No rendering, no UI.
2. Completion in the editor for all four triggers, over the parsed library.
   This alone is the daily improvement, and it needs nothing else.
3. The `citations` engine feature and module: Pandoc citation syntax,
   Hayagriva formatting, the reference list, and the anchoring test.
4. `komodoc bib pull`, then the local app's Zotero discovery and the
   add-from-search gesture.

Steps 1 and 2 are worth shipping alone. A document that formats its own
citations is a bigger change and should not hold up completion.

## Tests

Rust, `crates/engine/src/bib.rs`: parsing BibTeX and BibLaTeX
fields, `@string` macros, cross-references and braced titles; a malformed
entry skipped with a diagnostic while its file still parses; duplicate keys
across two files resolved in path order with a diagnostic; a front-matter
`bibliography:` narrowing the library; a named file missing from the tree.

Rust, citation rendering: each Pandoc citation shape against a fixed library
and a fixed style, including locators, prefixes, suffixes and multiple keys;
an unresolved key rendering as itself with a diagnostic; the reference list at
a `# References` heading and at the end without one; style-specific author,
year, delimiter, and locator output with bundled locales.

Rust, `crates/komodoc/src/tests/bib_cli.rs`: `bib pull` writing a
deterministic file, a second pull adding one entry producing a one-entry diff,
and a pull against an unreachable Zotero failing with setup guidance rather
than an empty file.

Web, `web/checks/bibliography-wasm.mjs`: compiled ABI parsing and rendering,
cache invalidation, module sizes, a 1,000-entry timing sample, fetch retries,
and comment anchoring across citation-style changes. The Chromium check in
`web/checks/citations-browser.mjs` exercises title search and key insertion.

Web, `web/checks/citations.mjs`, as pure functions: the completion matcher's
ranking over key, author, title and year; the insertion for each trigger; the
empty-library case typing a literal `@`.

A source-anchoring test covering a comment on a sentence containing a citation,
surviving a change of style, belongs with the existing anchoring tests.

## Open questions

- **Key stability across pulls.** Zotero's own citation keys are a Better
  BibTeX concept, not a Zotero one. Which key a pulled entry gets, and whether
  it can change under an author who has already cited it, needs deciding
  before the pull command is written; a pull that renames keys silently breaks
  every citation in the document.

## References

- [wasmtex.md](wasmtex.md) -- BibTeX and Biber routing for LaTeX, local
  discovery and the bridge protocol.
- [quarto.md](quarto.md) -- the Quarto dialect, its citation step and the
  injected-text anchoring question.
- [catalog.md](catalog.md) -- secrets, retention and erasure, which a
  server-side sync would have to satisfy.
