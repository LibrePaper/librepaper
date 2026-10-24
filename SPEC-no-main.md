# No main file

A project has no main file. The preview shows the file open in the source.

## Behaviour

- The preview follows the file open in the source. Switching files switches the preview.
- A file that renders on its own previews itself.
- A file that cannot render on its own, and that exactly one file includes, previews inside that file. A chip in the preview says `intro.tex, shown inside thesis.tex`.
- A file that cannot render on its own, and that no file or several files include, shows a card in the preview. The card says why it has no preview. It lists the files that include it, each with a button that locks the preview on that file.
- The eye in Files locks the preview on a file. The source can then move to any file and the preview stays on the locked one, rendering on every change in any file.
  - The locked row shows the filled eye. Other rows that render on their own show a faint eye button.
  - The preview shows a chip `Locked: book.tex` with a close button. Closing it, or pressing the eye again, unlocks.
  - The lock lasts for the visit. It is not stored and nobody else sees it.
- A file with no renderer at all (`.bib`, `.csv`, `.py`) is treated like an included file with no includer: the card says it has no preview.

## Which file renders on its own

| Format | Renders on its own when |
|---|---|
| LaTeX | it has `\documentclass` outside a comment (subfiles chapters count) |
| Typst | no other `.typ` file has `#include` or `#import` naming it |
| Quarto, Markdown | its name does not start with `_` and no file has `{{< include >}}` naming it |
| anything else | never |

A file includes another when it names it with `\input`, `\include`, `\subfile` (LaTeX, `.tex` implied), `#include`, `#import` (Typst) or `{{< include >}}` (Quarto, Markdown). Paths resolve against the including file's folder, then the project root for LaTeX. The rules read text only.

LaTeX resolves an included name against the including file's folder first, then the project root, adding `.tex` when the name has no extension. Typst and Quarto resolve against the including file's folder, and a Typst path starting with `/` against the root. A Typst `@preview/...` import names a package, not a file. LaTeX comments start at an unescaped `%`. A file never includes itself.

When the open file cannot render on its own, the preview walks up its includers while each step has exactly one includer, and stops at the first file that renders on its own. Any step with zero or several includers ends the walk on the card. A cycle ends it too.

`web/tests/fixtures/opening.json` pins these rules. The browser (`web/src/lib/opening.js`) and the server (`crates/librepaper-document/src/document/opening.rs`) both pass it.

## The file a project opens on

Paths sort case-insensitively, ties broken by byte order. The project opens on the first match of these, in order:

1. A top-level file that renders on its own, is not included by another file, and is not named `README.*` or `LICENSE.*`.
2. The same over the whole tree.
3. Any file that renders on its own, top level first.
4. Any text file, top level first.

An empty project opens on nothing. Link visitors open on the same file and can switch files.

## Render trees

A tree handed to a renderer still names the file it builds in its `main` field. That field is the render's entry, chosen per render by the preview, and is not a project setting. The projection has no main. Its digest keeps the `librepaper.projection.v1` byte layout with an empty entry line, so the digest of a render tree for a given file is unchanged and comment render digests stay valid. Projection digests (labels, etags) change once.

## What used to be main

| Job | Now |
|---|---|
| File a project opens on | the opening rule above |
| Download and export | the file the preview shows |
| Agent `render` tool | a required `path` |
| Desktop app builds | the file the preview shows |
| Project list format | the format of the opening file, computed where the catalog row is written |
| Templates and new projects | nothing to set |
| "Set as main file", "no main file" card | deleted |

## Local preview cache

The page each file last drew is kept in IndexedDB (`librepaper-preview-v1`), keyed by project and path. It is painted the moment the preview moves to that file, and the render that follows replaces it. A lookup also sees pages still in the five second write pause. At most 48 pages and 96 MiB are kept. Nothing goes to the server.

## Cutover

- The Loro `meta.main` key is no longer read or written. Existing values are ignored.
- The catalog `main_path` column is dropped by a new migration. The listing format comes from a derived column if the list needs one.
- No compatibility shim. Old clients are not supported.
