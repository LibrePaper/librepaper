---
title: "Edit in the browser"
---

Edit markdown or typst documents with source, rendering, and comments visible; panes fold away.

**Files** sidebar: create/upload files, drag to move, drop to upload. **File** menu: downloads, Share, History. **Download PDF/HTML** exports result; **Download project** (owners/editors only) saves ZIP.

**Outline** sidebar: headings in the open file, indented by level. Click to open section. Supports Markdown, Quarto, Typst, HTML, LaTeX.

Right-click or use **⋯**: rename, move, download, delete, duplicate. Ctrl/Cmd-click: select multiple. F2: rename. Delete: confirm deletion. Moves preserve co-editing. References not rewritten. Main file folder cannot delete until another file is main.

Editors have source access; readers see live rendering. Edits save automatically; no separate published version. Comments retain source checkpoint and passage; moved text updates display but not target. Deleted words shown in discussion.

Multiple editors use CRDT; concurrent typing converges without waiting. Status row shows active editors. Server keeps all updates; closing tabs loses nothing. Editors sync source; readers render locally, refetching only changed files.

Checkpoints recorded on: named snapshot, restore, CLI commit, proposal accept. History panel: view earlier versions, compare changes, restore version or passage.

Rendering on client devices; server never compiles. Readers see rendered output only; private inputs (data files, bibliographies) not sent to reader browsers.

The formats, and they are not available in the same places:

| Source | Editor renderer | Editor compiler download |
|---|---|---|
| Markdown | comrak | ~130 KB compressed |
| Quarto | Markdown draft; optional local Quarto render | Reuses the Markdown renderer |
| Typst | typst | ~13 MB compressed |
| HTML | the identity | nothing |
| LaTeX | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

Renderers compiled to WebAssembly; no installation needed. Same compiler for editors and readers ensures consistency. Cached for one year.

HTML preview default (faster); **View > Typst PDF preview** for print layout. HTML supports text, tables, citations, images, MathML but not all PDF formatting. Compile errors show diagnostics; source stays accessible.

HTML is its own source (identity renderer). Quarto (`.qmd`): readers see Markdown-draft rendering, code never executed. Local Quarto render through companion only.
