---
title: "Edit in the browser"
---

Edit Markdown, Quarto, Typst, HTML and LaTeX documents with source, rendering, and comments visible side-by-side. Either pane folds away.

## Files

Owners and editors can download the whole project as a ZIP. Readers and commenters cannot use that project-download control, but their browser still receives the projected source files and shared assets needed to render the document. Do not put sensitive material in a shared project on the assumption that a reader can see only its rendered page.

- File moves preserve collaborative text editing; references in source files are not rewritten.
- A folder containing the main file cannot be deleted until another file becomes main.

## Saving and co-editing

Edits save automatically; CRDT ensures concurrent typing converges without waiting. The server holds the document source and relays updates, so closing the last tab loses nothing. Readers see the same live rendering as editors. They do not join CRDT synchronization or receive the operation history, but the reader app fetches a source projection (text, main-file metadata, and shared assets) to render the document. That source is readable by authorized readers in their browser; keep private inputs outside the shared project. See [rendering and live sync](../architecture/rendering.html) for the delivery boundary.

Comments retain their source checkpoint and passage; moved text updates the display but not the target.

## Formats

| Source | Editor renderer | Editor compiler download |
|---|---|---|
| Markdown | comrak | ~130 KB compressed |
| [Quarto](../notebooks/quarto.html) | Markdown draft | Reuses the Markdown renderer |
| Typst | typst | ~13 MB compressed |
| HTML | the identity | nothing |
| LaTeX | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

Browser renderers run in WebAssembly on editors' and readers' devices; the server does not compile documents. The reader receives the shared source projection and assets needed for rendering, not the full collaboration log. A companion can run local tools against a folder that is not uploaded. Readers refetch only files whose digest changed.

## Preview modes

HTML previews are the default. For Typst, HTML does not reproduce all PDF formatting; some templates require HTML-specific show rules.
