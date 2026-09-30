---
title: "Edit in the browser"
---

Edit Markdown, Quarto, Typst, HTML and LaTeX documents with source, rendering, and comments visible side-by-side. Either pane folds away.

## Files

The **Files** sidebar shows a folder tree.

- Create files and folders using the toolbar; upload from your computer with drag or drop.
- **File** menu at the top offers downloads, Share, and History shortcuts.
- **Download PDF** or **Download HTML** exports the displayed result.
- **Download project** (owners and editors only) saves all source and input files as a ZIP.
- Drag files to move them; drop on empty space to move to the top level.
- Upload name collisions offer Keep both or Skip.
- Empty folders survive browser reloads and appear in ZIP downloads.
- Right-click or use **⋯** to rename, move, download, delete, or duplicate items.
- Use Ctrl/Cmd-click or Shift-click to select multiple items.
- Press F2 to rename; Delete asks for confirmation before deleting.
- Moves preserve collaborative text editing; references in source files are not rewritten.
- A folder containing the main file cannot be deleted until another file becomes main.

## Outline

The **Outline** sidebar lists headings in the open file, indented by level.

- Click a heading to jump to that section in the editor.
- The outline updates as you and your collaborators edit.
- Supports Markdown, Quarto, Typst, HTML, and LaTeX.

## Saving and co-editing

Edits save automatically. Several people can edit at once; the source uses CRDT so concurrent typing converges without either person waiting.

- The status row shows how many editors are in the session.
- The server holds the document source and relays every update, so closing the last tab loses nothing.
- Whoever opens the document next joins what is there.
- Readers see the same live rendering that editors do; they receive no editable source or project files.
- Comments retain their source checkpoint and passage; moved text updates the display but not the target.

## History

Checkpoints are recorded when someone names one, when a restore lands, when the command line commits, or when a proposal is accepted.

- The history panel lets you read earlier versions, compare changes, and restore a whole version or bring back individual passages in the editor.
- There is no automatic timer; a plain comment does not add a checkpoint.

## Rendering

Rendering happens on editors' and readers' own devices; the server never compiles a document.

- Readers see rendered output only; private inputs such as data files and bibliographies never reach reader browsers.
- Editors sync source through the socket and render previews locally.
- Readers render the same source locally, refetching only files whose digest changed since they last had it.

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

HTML previews are the default and fastest.

- **View → Typst HTML preview (experimental)** is what a document starts on.
- Use **Typst PDF preview** to check the printed layout.
- The choice is remembered for this document in this browser.
- HTML supports text, tables, citations, embedded images, and MathML equations, but does not reproduce all PDF formatting; some templates require HTML-specific show rules.
- PDF and HTML exports are generated again on demand.
- A compile error keeps source access available while showing diagnostics; editors can retry in the browser or use their local companion.

For HTML format, the renderer is the identity: the page shows exactly what was written. This covers everything Quarto, Jupyter, and marimo produce, so editing, co-editing, and comments reach the documents most papers arrive in.

The `.qmd` is the durable Quarto source. Readers see the browser's Markdown-draft rendering of it, the same subset an editor sees without running Quarto; code is never executed for a reader. A local Quarto render with real computed output appears only on the machine that produced it, through the companion.
