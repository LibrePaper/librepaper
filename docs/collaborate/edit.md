---
title: "Edit in the browser"
---

A document opened from markdown or typst keeps that source, so it can be
edited in the page it is read in: the source on one side, the rendered
document on the other, and the comments beside both. Either of the two panes
next to the source folds away.

The **Files** sidebar is a folder tree. Its toolbar creates files and folders
inside the selection, or uploads files from your computer; the **File** menu
at the top of the page offers the same, along with downloads and shortcuts to
the Share and History panels. **Download PDF** or **Download HTML** exports the
displayed result to the user's computer. **Download project**, available only
to owners and editors, saves every source and input file as a ZIP. Drag files or
folders onto another folder to move them; drop onto empty space in the sidebar
to move them to the top level. Dropping files or directories from your computer uploads them with
their folder structure. Upload name collisions offer **Keep both** or **Skip**.

The **Outline** sidebar lists headings in the open source file, indented by
section level. Click a heading to open the source pane at that section. The
outline updates as you and your collaborators edit, and supports Markdown,
Quarto, Typst, HTML, and LaTeX files.

Right-click an item or use its **⋯** menu to rename, move, download, or delete
it; files can also be duplicated. Use Ctrl/Cmd-click or Shift-click to select
several items, F2 to rename, and Delete to open the deletion confirmation.
Empty folders survive browser reloads and are included in browser ZIP
downloads. Moves preserve collaborative text editing, but references in source
files are not rewritten. A folder containing the main file cannot be deleted
until another file is made the main file.

The editor is offered to whoever may replace the document, and the document
opens ready to work on. Edits save automatically. Readers and commenters see
the same document editors do, rendered live in their own browser as it is
edited; they receive no editable source or project files, but there is no
separate published version to fall behind. Saving and compiling previews
happen the same way for everyone.

Comments retain their source checkpoint, source passage, and the rendering on
which they were made. Edits move their displayed source attachment without
changing what they refer to; comments whose words can no longer be found in
the current text still retain their discussion and say so, rather than
silently pointing at the wrong passage. Only the current rendering is shown,
so an old comment does not carry an old page along with it.

Several people can edit at once. The source is a CRDT, so two people
typing in the same sentence converge without either waiting for the other, and
the status row under the toolbar says how many are in the session. The server holds the document source,
relays every update and keeps it, so closing the last tab loses nothing
and whoever opens the document next, in a browser or from the terminal, joins
what is there.

Editors synchronize source through the socket and render previews locally.
Readers render the same source locally too, refetching only the files whose
digest actually changed since they last had it. The origin pays for source
transfer between editors, that projection delivery to readers, collaboration,
persistence, and history maintenance.

History is kept for you. The server records a checkpoint when somebody names
one, when a restore lands, when the command line commits, or when a proposal
is accepted -- and at no other time: there is no timer, and unlike those, a
plain comment does not add one, since it carries its own record of the moment
it was made. An explicit restore records a new event of its own. The history
panel lets you read earlier versions, compare changes, and restore a whole
version or bring back individual passages in the editor.

Rendering happens on editors' and readers' own devices; the server never
compiles a document. Source APIs, synchronisation state, private assets, and
source history require editor access. A rendered document and everything
embedded in it are readable and downloadable by anyone who can open it;
private inputs -- bibliographies, data files, anything the compiler reads but
the page does not show -- are never sent to a reader's browser at all.

The formats, and they are not available in the same places:

| Source | Editor renderer | Editor compiler download |
|---|---|---|
| Markdown | comrak | ~130 KB compressed |
| Quarto | Markdown draft; optional local Quarto render | Reuses the Markdown renderer |
| Typst | typst | ~13 MB compressed |
| HTML | the identity | nothing |
| LaTeX | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

Both renderers are the same crate the binary itself renders with, compiled to
WebAssembly. Nothing else has to be installed: opening a `.typ` file needs no
`typst` binary on your PATH, because the compiler is inside LibrePaper, and it
is the same one both editors and readers run, so a document cannot render one
way for one of them and another way for the other.

The Typst module contains the compiler and embedded fonts. Renderer URLs include
their content digest and are cached for a year.

Every format previews as HTML by default, Typst and LaTeX included: a flowing
page reflows to the pane it is read in and arrives without waiting for a paged
compile. Typst's paged PDF exporter preserves page layout, columns, headers,
footers, and typography, and is one choice away. The shared PDF viewer supplies
selectable text for comments and highlights. Source navigation matches visible text; generated
text and formulas can have no match. Existing project-file and package
resolution limits still apply.

**View → Typst HTML preview (experimental)** is what a document starts on; use
**Typst PDF preview** to check the printed layout. The choice is remembered
for this document in this browser. HTML supports semantic text, tables,
citations, embedded images, and MathML equations, but does not reproduce all
PDF formatting; some templates require HTML-specific show rules. It always
uses the browser compiler, including when Calepin is selected for PDF previews.
HTML previews are transient client results. PDF and HTML exports are generated
again on demand, and a compile error keeps source access available while
showing diagnostics; editors can retry in the browser or use their local
companion.

The Typst renderer is fetched with the other pinned browser modules as part of
`make build`, so every deployment serves the same four renderer interfaces.

A document created in HTML format is its own source, and its renderer is the
identity: it is shown exactly as written, and it opens in the editor like the
other two. That covers everything Quarto, Jupyter and marimo produce, so the
live preview, the co-editing and the comments reach the documents most papers
actually arrive in. Editors may change raw HTML source, and a reader sees
that source live, the same as any other format.

The `.qmd` is the durable Quarto source. What a reader sees is the browser's
Markdown-draft rendering of it, the same subset an editor sees without
running Quarto -- code is never executed for a reader. A local Quarto render
with real computed output is visible only on the machine that produced it,
through the companion.
