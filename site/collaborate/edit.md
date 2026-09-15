---
title: "Edit in the browser"
---

A document published from markdown or typst keeps that source, so it can be
edited in the page it is read in: the source on one side, the document as it will be
published on the other, and the comments beside both. Either of the two panes
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
the last explicitly published HTML version, and receive no editable source or
project files. In **Share → Published version**, choose **Publish** or **Publish
update** to prepare a new version for them. The toolbar's **Unpublished changes**
indicator opens that section. Saving and compiling previews do not publish.

Comments retain their selected quotation and publication identity. When a new
version is published, passages are located again where possible; unmatched
comments retain their discussion and indicate an earlier publication. Only the
current full rendering is retained, so old comments do not preserve an old page.

Several people can edit at once. The source is a CRDT (Yjs), so two people
typing in the same sentence converge without either waiting for the other, and
the status row under the toolbar says how many are in the session. The server holds the document source,
relays every update and keeps it, so closing the last tab loses nothing
and whoever opens the document next, in a browser or with `librepaper sync`, joins
what is there.

Editors synchronize source and render previews locally. Explicit publication
uploads compressed HTML and only missing public display assets. Readers reuse
unchanged assets and refresh deliberately when a newer publication is available.
The origin pays for source transfer between editors, publication delivery,
collaboration, persistence, and history maintenance.

History is kept for you. The server takes a checkpoint of the source when the
document has been quiet for a while, when the last editor leaves, when someone
comments, and whenever `librepaper publish` writes to it. Unchanged text reuses its
checkpoint; an explicit restore records a new event. The history panel lets you
read earlier versions, compare changes, and restore a whole version or bring
back individual passages in the editor.

Rendering happens on editors' devices. The server stores one current published
HTML bundle per document and serves it to readers and commenters; it does not
compile documents. Source APIs, synchronization state, private assets, and source
history require editor access. Published HTML and everything embedded in it are
readable and downloadable; private inputs must be excluded from the display bundle.

The formats, and they are not available in the same places:

| | Source uploaded with | Editor renderer | Editor compiler download |
|---|---|---|---|
| **Markdown** | `librepaper publish paper.md` | comrak | ~130 KB compressed |
| **Quarto** | `librepaper publish paper.qmd` | Markdown draft; optional local Quarto render | Reuses the Markdown renderer |
| **Typst** | `librepaper publish paper.typ` | typst | ~13 MB compressed |
| **HTML** | `librepaper publish paper.html` | the identity | nothing |
| **LaTeX** | `librepaper publish paper.tex` | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

Both renderers are the same crate the binary itself renders with, compiled to
WebAssembly. Nothing else has to be installed: publishing a `.typ` file needs
no `typst` binary on your PATH, because the compiler is inside LibrePaper, and it
is the same one the editor runs, so a document cannot render one way when it
is published and another way when it is edited.

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

A document published as HTML is its own source, and its renderer is the
identity: it is shown as it was published, which it always was, and it opens in
the editor like the other two. That covers everything Quarto, Jupyter and
marimo produce, so the live preview, the co-editing and the comments reach the
documents most papers actually arrive in. Editors may change raw HTML source;
readers see only its last explicitly published version.

The `.qmd` is the durable Quarto source. Previews remain local until an editor
explicitly publishes an HTML display bundle.
