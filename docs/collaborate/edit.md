---
title: "Edit in the browser"
---

Edit Markdown, Quarto, Typst, HTML and LaTeX documents with source, rendering, and comments visible side-by-side. Either pane folds away.

## Files

Owners and editors can download the whole project as a ZIP; others get only the rendered PDF or HTML.

- File moves preserve collaborative text editing; references in source files are not rewritten.
- A folder containing the main file cannot be deleted until another file becomes main.

## Saving and co-editing

Edits save automatically; CRDT ensures concurrent typing converges without waiting. The server holds the document source and relays updates, so closing the last tab loses nothing. Readers see the same live rendering as editors but receive no editable source or project files.

Comments retain their source checkpoint and passage; moved text updates the display but not the target.

## Formats

| Source | Editor renderer | Editor compiler download |
|---|---|---|
| Markdown | comrak | ~130 KB compressed |
| [Quarto](../notebooks/quarto.html) | Markdown draft | Reuses the Markdown renderer |
| Typst | typst | ~13 MB compressed |
| HTML | the identity | nothing |
| LaTeX | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

All formats are compiled to WebAssembly; no installation is needed. The same compiler runs in both editors and readers, so with the same source, renderer and settings, a document renders the same way for everyone. Rendering happens on editors' and readers' own devices; the server never compiles a document. Private inputs such as data files and bibliographies never reach reader browsers. Readers refetch only files whose digest changed.

## Preview modes

HTML previews are the default. For Typst, HTML does not reproduce all PDF formatting; some templates require HTML-specific show rules.
