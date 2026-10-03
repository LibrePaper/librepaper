---
title: "Edit in the browser"
---

Edit Markdown, Quarto, Typst, HTML and LaTeX documents with source, rendering, and comments visible side-by-side. Either pane folds away.

## Files

The **Files** sidebar shows a folder tree. Create files and folders, upload from your computer. Owners and editors may download the whole project as a ZIP; others see Download PDF or HTML (the rendered result).

- File moves preserve collaborative text editing; references in source files are not rewritten.
- A folder containing the main file cannot be deleted until another file becomes main.

## Outline

The **Outline** sidebar lists headings, indented by level, and updates as you and collaborators edit. Supports Markdown, Quarto, Typst, HTML, and LaTeX.

## Saving and co-editing

Edits save automatically; CRDT ensures concurrent typing converges without waiting. The server holds the document source and relays updates, so closing the last tab loses nothing. Readers see the same live rendering as editors but receive no editable source or project files.

Comments retain their source checkpoint and passage; moved text updates the display but not the target.

## History

Checkpoints are recorded when someone names one, when a restore lands, when the command line commits, or when a proposal is accepted. There is no automatic timer; a plain comment does not add a checkpoint.

## Rendering

Rendering happens on editors' and readers' own devices; the server never compiles a document. Readers see rendered output only; private inputs such as data files and bibliographies never reach reader browsers. Readers render the same source locally, refetching only files whose digest changed.

## Formats

| Source | Editor renderer | Editor compiler download |
|---|---|---|
| Markdown | comrak | ~130 KB compressed |
| Quarto | Markdown draft; optional local Quarto render | Reuses the Markdown renderer |
| Typst | typst | ~13 MB compressed |
| HTML | the identity | nothing |
| LaTeX | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

All formats are compiled to WebAssembly; no installation is needed. The same compiler runs in both editors and readers, so with the same source, renderer and settings, a document renders the same way for everyone.

## Preview modes

HTML previews are the default and fastest. Typst documents support both HTML preview (experimental) and PDF preview for checking printed layout. HTML supports text, tables, citations, embedded images, and MathML equations, but does not reproduce all PDF formatting; some templates require HTML-specific show rules. A compile error keeps source access available while showing diagnostics.

For HTML format, the renderer is the identity: the page shows exactly what was written. The `.qmd` is the durable Quarto source; readers see the browser's Markdown-draft rendering without executing code. Computed output appears only on the machine that produced it, through the companion.
