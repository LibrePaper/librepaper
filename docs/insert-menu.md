# Insert menu

The editor navbar offers one Skeleton menu for LaTeX, Typst, Markdown and Quarto. The active file determines the syntax, including when an author switches between formats inside one project. Non-document files and positions inside comments, code or front matter disable document insertions. Inside math, matrices and cases insert their inner expression.

The 35 actions cover headings, abstracts, appendices, contents, figures, tables, citations, bibliographies, cross-references, anchors, inline/display/aligned/gathered math, cases, matrices, three list types, quotations, code, footnotes, links, seven scholarly blocks, page breaks, horizontal rules, columns and custom environments. Heading level supplies section/subsection/subsubsection variants.

Skeleton Dialog overlays collect dimensions, image paths/uploads, captions, labels, numbering, citation selections and locators, and other action-specific options. A source preview includes any setup edits before insertion. Simple actions wrap selected text or insert immediately; actions requiring setup show the preview first. Labels avoid collisions and Quarto labels receive the appropriate cross-reference prefix. Asset and bibliography paths are relative to the source that will resolve them.

## Format behavior

- LaTeX adds missing packages and theorem declarations to the main preamble. Required setup is applied together with the insertion, including from an included file. Existing natbib/biblatex conventions are respected. Narrative citations can add natbib and upgrade the standard plain bibliography style to plainnat. Insert a bibliography before citing if the document has no bibliography setup.
- Typst uses native headings, figures, tables, math, references and content functions. Scholarly blocks use figure kinds with their own counters. Custom environments require a defined content function.
- Quarto uses Pandoc/Quarto constructs and metadata, including contents, section numbering, bibliography setup, and typed cross-reference labels.
- Markdown uses the features available in LibrePaper's pinned renderer. Tables, footnotes and citations use supported Markdown syntax; anchors, description lists and layout blocks use HTML where needed. Math uses escaped `span[data-math-style]` content consumed by LibrePaper's existing KaTeX typesetter: the pinned WASM release does not parse dollar-delimited math. Ordinary Markdown headings and scholarly blocks do not acquire automatic numbering.

## Implementation

`web/src/lib/insert.js` owns the action registry, validation, syntax context, target discovery, collision handling and Markdown/Quarto generators. `insert-latex.js` and `insert-typst.js` provide the typesetting adapters. Builders return replacement text, a useful selection within that text, and explicit additional source edits.

`InsertMenu.svelte` owns menu and dialog state. `Reader.svelte` connects project assets, upload/preview helpers and edit permissions. `Editor.svelte` captures the selection with Yjs relative positions. It validates every edit before mutation, rejects changed selections or file/session switches, and applies the snippet and setup in one collaborative undo transaction. Cancellation releases the captured target.

## Verification

From `web/`:

```sh
npm run check:insert
npm run check:insert-renderers
npm run check
npm run build
```

The Insert checks cover format options and syntax contexts, render Markdown with LibrePaper's actual WASM module, validate math with KaTeX, and exercise Skeleton controls with Chromium. Compiler checks require `pdflatex`, `bibtex`, `typst`, and `quarto`; they compile generated documents and verify representative rendered features. The editor browser regression suite also checks existing collaborative editing and undo behavior.

## Review record

The review checked source generation, Skeleton dialog state, active-file detection, project setup edits, and collaborative undo. It found and fixed Quarto being treated as Markdown, the citation picker's array/object mismatch, disabled metadata-only submissions, package-ordering errors, incomplete custom-environment arguments, and end-of-file footnote placement.

- Critical/major findings: resolved and covered by focused checks.
- Minor findings: no remaining release blocker.
- Positive findings: a shared semantic registry, explicit setup previews, format compiler tests, and validation before atomic Yjs changes.
- Questions requiring author input: none.
- Verdict: approved after the checks above passed. The reference skills required no changes.
