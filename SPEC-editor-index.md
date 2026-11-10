# Editor document index

One format-neutral index of what a source file contains, shared by every
editor feature. Each format writes one adapter that reads its syntax; every
feature reads the index and never reads syntax itself.

## Why

Today each feature parses each format with its own regular expressions.
Labels alone are found five times (`latexLabels`, `markdownLabels`,
`quartoLabels`, `typstLabels`, `numberLabels`). The outline (`outline.js`),
spellcheck skip ranges (`spellcheck/*.js`), cited keys (`citation-keys.js`),
word count and the visual-markup facts (`visual-markup/*facts.js`) each repeat
the split. A new feature costs one implementation per format, and the copies
disagree at the edges.

With an index, a feature costs one implementation, and a format costs one
adapter.

## Two layers

1. **The editor tree**, per open file, updated on every keystroke by
   CodeMirror: colouring, folding of the visible file, bracket and environment
   matching, indentation, the context at the cursor. Each format supplies a
   CodeMirror language. This spec does not change that layer.
2. **The project index**, per file, rebuilt when typing pauses and merged
   across the project: everything that crosses files or needs names. This spec
   is about this layer.

## One grammar per format

Every format follows one rule. It has one grammar that recognizes every piece
of syntax any feature cares about. Consumers read the tree. None of them
searches source text for syntax.

Four layers make the rule concrete:

| Layer | Job | Per format |
|---|---|---|
| Grammar | all structure and syntax | one grammar: Lezer, or `typst-syntax` for Typst |
| Catalog | meaning: command signatures, heading levels, accent marks, widget kinds | data tables |
| Tree→Token mapper | turns nodes plus the catalog into the visual layer's Token model | a thin walker, `web/src/lib/visual-markup/languages/<format>/tree.ts` |
| Other views | highlighting `styleTags`, index adapter, spellcheck, outline, facts | each a walk over the same tree, from `web/src/lib/source-trees.js` |

**Reading text.** A consumer may interpret the text of a single node: an
environment name, a fence info string, an attribute list, a `#| key: value`
line in a code cell, or the character after an accent control symbol (TeX's
own next-token rule). A consumer never scans across nodes looking for
constructs. If a feature needs a construct the grammar lacks, the construct
goes into the grammar.

**TeX's macro-dependent syntax.** The grammar emits the lexical pieces:
control sequences, groups, brackets, ligature and tie tokens. Whether `[...]`
after a command is an optional argument depends on the macro's signature, so
the catalog decides it, in the walker.

**Regression net.** Visual tokens are pinned by the golden corpus in
`web/tests/fixtures/visual-tokens/`, checked by
`web/tests/unit/visual-tree-tokens.mjs`. After an intended change, run
`web/tools/visual-golden.mjs` and review the golden diff.

**Adding a format**, in this order:

1. Write the grammar. It is ours, under MIT. Never bundle a GPL grammar.
2. Wire it into `source-trees.js`.
3. Add the catalog, the mapper, the `styleTags`, the index adapter and the
   spellcheck skips.
4. Add corpus files to the golden set.

Org mode is next. It has an index adapter (`adapters/org.js`) and a
highlighter (`web/src/lib/org-language.js`) but no grammar, so it starts at
step 1.

## The index

A file's index is a list of items. Positions are UTF-16 offsets into the
file's text, which is what CodeMirror and JavaScript strings use. Lines are
computed on demand, never stored.

```js
/** @typedef {{
 *   kind: "heading" | "label" | "ref" | "cite" | "include" | "asset"
 *       | "block" | "math" | "code" | "definition" | "bibliography",
 *   range: [number, number],      // the whole construct
 *   name?: string,                // label id, key, path, macro or block name
 *   nameRange?: [number, number], // just the name, for navigation and rename
 *   level?: number,               // heading depth, 1 = top
 *   title?: string,               // heading or caption text, plain
 *   namespace?: "label" | "cite" | "either", // for ref and cite
 *   attrs?: Record<string, string>,          // format extras, e.g. block kind
 * }} IndexItem */

/** @typedef {{ path: string, format: string, items: IndexItem[] }} FileIndex */
```

Kinds:

| Kind | Meaning | LaTeX | Typst | Markdown / Quarto |
|---|---|---|---|---|
| `heading` | a section, with `level` and `title` | `\section{}` … | `= …` | `# …` |
| `label` | a name something can refer to | `\label{x}` | `<x>` | `{#x}`, `#\| label: x`, heading ids |
| `ref` | a use of a label | `\ref{x}`, `\cref{x}` | `@x` | `@fig-x` |
| `cite` | a use of a bibliography key | `\cite{k}` | `#cite(<k>)` | `[@k]` |
| `include` | another source file pulled in | `\input`, `\include` | `#include` | `{{< include >}}` |
| `asset` | a non-source file used | `\includegraphics` | `image()` | `![](…)` |
| `block` | a foldable structured region, `name` = env or kind | `\begin…\end` | `#figure[…]` | `:::`, code cells |
| `math` | a math region | `$…$`, `equation` | `$…$` | `$…$` |
| `code` | code or verbatim, not prose | `verbatim` | raw | fences |
| `definition` | a user definition | `\newcommand` | `#let` | — |
| `bibliography` | a bibliography file in use | `\addbibresource` | `#bibliography` | `bibliography:` |
| `markup` | syntax that is not prose, with `attrs.prose = "skip"` | commands | `#` calls | attributes, front matter |

**Ambiguous `@x`.** In Typst and Markdown, `@x` can be a label reference or a
citation. The adapter emits `ref` with `namespace: "either"`; the project
index resolves it: a label named `x` anywhere in the project makes it a
reference, otherwise it is a citation. Adapters never look outside their file.

**Prose.** Prose ranges are the file minus `code`, `math`, and the syntax the
adapter marks as markup. The adapter returns them through the same list as
`attrs: { prose: "skip" }` items, so spellcheck, Harper and word count read one
source.

## Adapters

```js
/** @type {(text: string, path: string) => IndexItem[] | Promise<IndexItem[]>} */
```

- Pure: same text, same items. No project access, no side effects.
- Per file, so results are cached by file and text identity.
- May be asynchronous, so the Typst adapter can ask the renderer worker.
- One adapter per format: `latex`, `typst`, `markdown` (Quarto is Markdown
  plus Quarto items, in the same adapter), `html`.

The first adapters wrap today's scanners. Better parsers (the Markdown tree
with Pandoc extensions, our own LaTeX grammar, Typst's parser in the worker)
later replace an adapter's inside without touching any feature.

## The project index

Holds the latest `FileIndex` per path and answers lookups:

- `definitions(namespace, name)` → items, across files
- `references(namespace, name)` → items, across files
- `file(path)` → `FileIndex`
- `names(namespace)` → every defined name, for completion

It reindexes a file when its text changes and the editor's edit pause fires
(the same cadence the visual-markup facts use today), and only that file.

## Queries

Named after LSP's, so a language server can later answer them for a format.

| Query | Answer |
|---|---|
| `definition(path, pos)` | the label, file or definition a ref, cite, include or asset at `pos` points to |
| `references(path, pos)` | every use of the name at `pos` |
| `rename(path, pos, newName)` | a list of `{ path, from, to, insert }` edits over every `nameRange` |
| `hover(path, pos)` | the target's `title` (heading or caption) and location |
| `symbols(path)` | headings, then labels, for the outline |
| `foldingRanges(path)` | `block` and heading spans |
| `diagnostics(path)` | refs with no label, includes and assets with no file, cites with no entry |
| `proseRanges(path)` | ranges for spellcheck, Harper and word count |

Format-specific writing stays with each format, as small functions:
`writeRef(name)`, `writeCite(keys)`, `writeLabel(name)`. Quick fixes and
inserts use them; nothing else builds syntax.

## Editor integration

New CodeMirror extensions, each reading only the index:

- **Go to definition:** Mod-click and F12. Opens the target file when needed,
  through the existing `openIncluded` path.
- **Find references:** Shift-F12, listed in a panel.
- **Rename:** F2, labels first. Edits go through `track-changes.js` when
  tracked changes are on, and through the session document for files not on
  screen, so collaborators receive ordinary edits.
- **Diagnostics with quick fixes:** a lint source using `@codemirror/lint`
  actions: replace an unknown name with the nearest defined one (by edit
  distance), or, for LaTeX, add the `\usepackage` that `latex-packages.js`
  says provides an unknown command.
- **Hover** for refs, beside the existing citation hover.

All user-visible text goes through Paraglide messages, per AGENTS.md.

## Code layout

```
web/src/lib/document-index/
  schema.js          typedefs above
  project.js         the project index and its lookups
  queries.js         the queries
  adapters/latex.js  adapters/typst.js  adapters/markdown.js  adapters/html.js
  writers.js         writeRef, writeCite, writeLabel per format
web/src/lib/document-index-extensions/
  navigation.js  rename.js  diagnostics.js  hover.js
web/src/lib/source-trees.js                          parsed tree per format, read by every view
web/src/lib/latex-language/                          our LaTeX grammar (Lezer) and its parser
web/src/lib/typst-language/                          Typst language support
web/src/lib/pandoc-markdown.js                       Pandoc and Quarto extensions to the Markdown parser
web/src/lib/visual-markup/languages/<format>/tree.ts tree-to-token mappers for the visual layer
```

## Migration

Each step ships on its own and leaves the app working.

1. **Schema and project index.** `schema.js`, `project.js`, `queries.js`,
   with unit tests on hand-written item lists. No adapter yet.
2. **Adapters from today's scanners.** Each adapter calls the existing
   functions and maps their results to items. An equivalence test per format
   asserts the adapter reproduces what `extractOutline`, the label functions,
   `citedKeys` and `skipRanges` return on the existing fixtures.
3. **Labels and references.** Go to definition, find references, rename and
   unknown-reference diagnostics, for every format at once.
4. **Move consumers.** Label completion (LaTeX `\ref`, Quarto `@`) and the
   citation checker's Typst labels read the index, through the editor's live
   index; `latexLabels`, `typstLabels` and `projectLabels` are gone. The
   outline, spellcheck regions and word count stay on `extractOutline` and
   `skipRanges`: those are the adapters' own scanners, so routing the
   consumers through the index would call the same code later, and spellcheck
   needs the live text of the open file, which the index trails by an edit
   pause. Quarto's label scan lives in `lib/quarto-labels.js`, shared by
   completion and the Markdown adapter.
5. **Better parsers.** The editor now has them:
   - **LaTeX:** our own Lezer grammar, `lib/latex-language/`. Its parser is
     generated with `bun run grammar:latex`. It provides highlighting, folding
     of environments and groups, indentation, `inMath` and auto-close of
     `\end{…}`.
   - **Markdown and Quarto:** Pandoc and Quarto extensions to the Markdown
     parser, `lib/pandoc-markdown.js`, covering dollar math, fenced divs,
     citations, cross references and attribute lists.
   - **Typst:** folding comes from the typst module's parse-only `folding`
     export, through the renderer worker (`lib/typst-folding.js`).

   The LaTeX and Markdown adapters read those trees for comments, verbatim,
   code, math, environments and divs, citations, labels and references. They
   keep `extractOutline` for heading titles and `skipRanges` for spellcheck,
   which other consumers share. The Typst adapter keeps its synchronous
   scanner. For the open `.typ` file, the module's parse-only `symbols`
   answer is laid over the scanner's headings and definitions in the live
   index (`setOverlay`), only while the text matches. The Outline panel shows
   the module's headings for that file once they have arrived.

## Non-goals

- Macro expansion or any LaTeX semantics beyond names. Heuristics are enough.
- Compiling. The index never runs an engine.
- Replacing format-specific completion vocabulary (LaTeX command lists, the
  Quarto schema, Typst's own completion). Those stay as their own sources.
- Visual-markup widgets. They may read the index for numbering later; that is
  a separate change.

## Licensing

LibrePaper is MIT. Adapters and grammars are ours. Copyleft parsers and
servers (the AGPL `codemirror-lang-latex` and `lezer-latex`, the GPL texlab)
are neither bundled nor read while writing ours; ours are written from format
documentation and our own tests. Permissive libraries (MIT, Apache-2.0, BSD)
may be used.

## Later

When something besides the browser editor needs structure (the server's MCP
tools, the CLI), the project index and adapters move to Rust, compiled to
WebAssembly for the browser and native elsewhere, behind the same queries.
The LaTeX part can then be published as a permissive language server.
