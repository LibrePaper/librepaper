# SPEC: directories, several files behind one document

Status: proposed. Nothing here is built. Extends the Rust host, the engine
ABI and the browser editor. Written against `01-SPEC-history.md`, whose
checkpoint it generalises, and against `04-SPEC-sync.md` and
`05-SPEC-latex.md`, both of which name this work as the thing they stop
short of. It reverses one line of the LaTeX spec's "decisions taken", stated
under "What changes elsewhere".

## The problem

A document is one text. The browser holds one `Y.Text` named `source`, the
server holds the same document and relays it, a checkpoint is the bytes of
that text, and the renderers are given that text and nothing else.

The engine is not so limited. `DocumentWorld` in `engine/src/typst.rs`
resolves every file typst asks for through a map the host fills, and on
the command line `render_typst_document` fills it from the file's own
directory, so `komodoc publish paper.typ` beside a `lib.typ` and a
`refs.bib` renders on the author's laptop. The WebAssembly module exports
`add_file` and `clear_files` for the same map, and the browser never calls
them; the comment on the map says it stays empty "until several files can
travel with one source". So a document that imports anything renders on
one machine and nowhere else, and since `01-SPEC-history.md` made readers
render for themselves, the reader gets the error the editor gets.

Three specs have walked up to this and stopped. `04-SPEC-sync.md` names it
as "a project rather than a file" and says the sync client can feed one
once it exists. `05-SPEC-latex.md` lists files beside the document under
its non-goals and offers `filecontents*` as the way a bibliography rides
along. `06-SPEC-html.md` keeps the note that a typst document needing files
beside it is published as HTML until the project exists. And
the engine fills a `file` field for spans in imported files
and admits that in the browser it is always empty.

LaTeX makes the gap the ordinary case rather than the edge. A paper is
`main.tex`, a `chapters/` directory, `refs.bib` and a `fig/` directory of
PNGs and PDFs. `filecontents*` can carry the bibliography. It cannot carry
a figure or a chapter, and a paper split into files is split for reasons
its authors will not give up for a review tool. The Overleaf research note
(`market-research/`, §2 and §7) lists multi-file projects, `.bib` as a
first-class file and upload-as-zip as table stakes, and it is right.

## The decision

Five rules.

**A document is a directory, and the reader never sees the directory.**
The slug, the URL, the rendered page, the comments and their anchors are
what they are today. A reader opens a document and sees one rendered
thing; a commenter highlights a sentence in it. The directory is an editing
concern: it is what the source pane shows, what a checkpoint records, and
what the renderer is given. A document with one file is a directory with
one file, and every existing document is one.

**Two kinds of file: texts and assets.** A text is a file a person edits --
the main file, a chapter, a `.bib`, a `.sty`. Texts live in the shared
Yjs document, one `Y.Text` per path, in the one room the document already
has. An asset is bytes nobody edits in place -- a PNG, a PDF figure, a font.
Assets live in the store, named by their digest, and are never put into the
CRDT. The shared document records which digest sits at which path, and
nothing more.

**One main file, declared and not discovered.** The directory is the set
of files the author put there, and one of them is the main file. Komodoc
does not parse `\include`, `\input` or `#import` to work out the tree,
because TeX cannot be resolved statically and a guess that is wrong is
worse than a list. A file the main file never reaches is still stored and
versioned, because a directory holds what is in it. A file the main file
reaches and the directory lacks fails with the error the compiler gives,
where it gives it.

**A checkpoint is the tree.** One checkpoint records every path and the
digest of what was at it, texts and assets alike, at one moment. Restoring
to Tuesday restores every chapter and the bibliography together; a chapter
and the file that includes it can never be recorded out of step. A file
unchanged between two checkpoints costs no new object.

**The ceilings are on the sum, and assets have their own.** The source
ceiling that bounds one text today bounds all the texts together. Assets
are where bytes actually go, so they get a ceiling of their own per
document, count against the owner's quota, and are pruned when nothing
retained refers to them.

## What a directory is

### Paths

A path is relative, `/`-separated, UTF-8, and normalised: no leading `/`,
no empty segment, no `.` or `..` segment, no segment beginning with `.`,
at most `max_path` bytes and at most eight segments. Two paths that
differ only in case are refused, not because the store minds but because
a sync client on macOS or Windows would write them to the same file. A
path is NFC-normalised on the way in, because macOS writes NFD, and the
same name would otherwise be two keys. The
server enforces all of this on every route that takes a path, and the
shell and the command line check first so a refusal is explained on the
laptop rather than by a status code.

### Kinds

Which kind a file is follows from its extension, and the lists are rules
in `config`, so a deployment can widen or narrow them without a build:

| rule | default | meaning |
| --- | --- | --- |
| `text_extensions` | `.tex .typ .md .markdown .bib .sty .cls .bst .txt .csv .json .yml .yaml .html .css` | texts, which must be valid UTF-8 |
| `asset_extensions` | `.png .jpg .jpeg .gif .svg .webp .pdf .otf .ttf .woff .woff2` | assets, which may be anything |
| `derived_extensions` | `.aux .bbl .blg .log .out .toc .fls .fdb_latexmk .synctex.gz .lof .lot .nav .snm` | refused |

Anything not on the first two lists is refused. A store that accepts any
file is a file host, and this project is not one. A `.pdf` is an asset
because figures are PDFs; the one PDF that is derived, the main file's own
output, is skipped by name by `publish` and `sync`, below.

### The main file

The shared document records which text is the main file, in the `meta`
map below, and the index entry mirrors it as `main`, the path, so that a
listing and `source_format` -- derived from its extension as it is derived
from the uploaded file's today -- need not open the session. The server
holds the document, so it reads `main` from the same place every peer
does. An earlier draft kept it on the index entry alone, which left a
rename of the main file in the session with no way to reach the entry,
and a change of main on the entry with no way to reach the peers.
Changing the main file is a set on `meta`, offered in the editor's file
list and by `publish --main`.

## In the session

### The shared document

The Yjs document holds four maps instead of one text:

| name | type | contents |
| --- | --- | --- |
| `files` | `Y.Map` of `Y.Text` | id to the text |
| `paths` | `Y.Map` of string | id to the path the text is at |
| `assets` | `Y.Map` of string | path to the digest of the asset at it |
| `meta` | `Y.Map` of string | `main`, the id of the main file |

Texts are keyed by an id, not by a path, and the path is a value beside
the text. An id is minted by whoever creates the text -- twelve random
characters, the way a comment id is -- and never changes. The reason is
what a rename would otherwise cost. Yjs has no rename: a text keyed by
its path is renamed by deleting one key and setting another with a copy
of the text, and every edit any peer makes into the old text before it
learns of the rename lands in a text no key reaches. That window is not
an instant. The browser keeps the document in IndexedDB and sends its
whole state on reconnect (`web/src/lib/collab.js`), so a coauthor who
edits a chapter for an hour on a train while it is renamed loses the
hour. With the path as a value, a rename is one set on `paths`, and the
edits in flight land in the text they were always in. Two peers creating
a file at the same path at the same moment get two texts with two ids
and a collision the server resolves, below, instead of one text silently
replacing the other.

Assets stay keyed by path: the value is a digest, the bytes are in the
store, and a set that loses to a concurrent set loses a name, not work.

`source` is retired. The server migrates a session the first time it loads
it under the new code: if `files` is empty and `source` is not, the text
moves into `files[<id>]` with `paths[<id>]` the main path and `meta.main`
the id, in one transaction, and the session state is rewritten. Nothing is
done to `history/`; see "Checkpoints". The rollback caveat `TODO.md` already
records for the layout migration applies here too and is not made worse.

One room, one socket, one awareness, one shared document: this is why the
files are a map in one document rather than a room each. A checkpoint of
the tree is a read of one document at one instant. Two people typing in
two chapters never touch the same `Y.Text`, so parallel editing of a
modular paper is free rather than a merge. And the socket protocol does
not change: `y-open`, `y-state`, `y-update`, `y-awareness`, `y-ack`,
`y-checkpoint` and `y-peers` carry a document with four maps in it as they
carried one with a text, and `04-SPEC-sync.md`'s table stands.

Creating a text is a `set` on `files` and on `paths` with a fresh id, in
one transaction; deleting is a `delete` from both; renaming is a `set` on
`paths` alone.

Awareness gains `file` beside `user`: the id of the text the peer's caret
is in.
The caret positions `y-codemirror.next` publishes are relative to the
`Y.Text` they were made in, so they already paint only in the right file;
`file` is for the file list, which shows who is in which chapter.

### The ceiling

`admit_update` in `session.rs` measures the document before applying an
update, and its measure becomes the sum of every text in `files` plus the
bytes of every key in both maps. The bound-then-rehearse shape is
unchanged. `max_html`, which `05-SPEC-latex.md` renames `max_document`,
bounds that sum: a paper split into thirty files is allowed exactly what a
paper in one file is allowed. A new rule `max_files` bounds the number of
keys across both maps; an update that would pass it is refused the way one
that passes the byte ceiling is.

### What the server refuses, and what it repairs

An update that would carry the texts past `max_document` or the keys past
`max_files` is refused the way an oversized update is refused today
(`session::admit_update`): it never touches the document, and the socket
that sent it is closed. That is the right answer for size and the wrong
one for everything else. A closed socket reconnects with backoff and sends
its whole state again, because that is what keeps a disconnection from
losing anything (`web/src/lib/room.js`), so an update the server will
never accept comes back on every reconnect for as long as the browser
keeps it. A tab holding the old bundle across the deploy, writing to
`source`, would be locked out until its IndexedDB was cleared; so would
any peer with one malformed path.

So a structural fault is admitted and repaired rather than refused. The
server applies the update, then in a transaction of its own puts the
document right, and relays that as the ordinary `y-update` it is. Four
repairs:

- a write to `source` after migration: what was written is folded into
  the main text the way `replace_text` folds a publish, and `source` is
  emptied;
- a path in `paths`, or a key in `assets`, that fails the path rules: the
  text is given a path made from its id, `unnamed-<id>.txt`, and the
  asset key is deleted, so nothing is lost and the file list shows what
  happened;
- two ids whose paths are the same, or differ only in case, which no
  per-update check can catch because each of two concurrent sets was
  valid on its own: the one the server applied second gets its path
  suffixed, `refs (2).bib`, and the file list shows both;
- a `meta.main` that names no text: reset to the first text by path.

The old bundle is why this belongs to step 1 and not to a later one: step
1 is what retires `source`.

### Readers

Readers join the session and receive the whole document today, because
they render it themselves. That stays true: the texts reach a reader
through `y-state`, and the assets reach them by URL, below. The transfer
above the socket message cap that `01-SPEC-history.md` defines is the
same path for a larger state.

## Assets

### Routes

| route | who | does |
| --- | --- | --- |
| `PUT /api/documents/<slug>/assets` | editor | body is the bytes; answers `{sha, size}` |
| `GET /api/documents/<slug>/assets/<sha>` | whoever may read the document | the bytes, `Cache-Control: public, max-age=31536000, immutable` |

The server hashes the body, refuses it above `max_asset` or when it would
carry the document past `max_assets` or the owner past their quota,
stores it at `assets/<slug>/<sha>`, and answers with the digest. The
client then sets `assets[path] = sha` in the shared document; the name is
the client's to give, the bytes are the server's to keep. A PUT of bytes
the document already has answers with the digest and stores nothing.
Uploads count against `uploads_per_hour` like any other upload.

The digest names the object under the slug and nowhere else. The same
figure in two documents is stored twice, on purpose: a blob shared across
documents has no owner to charge and no moment at which it may be
deleted, and the bytes are cheaper than the bookkeeping.

A `GET` honours the document's visibility under `03-SPEC-sharing.md`. The
frame on the documents origin fetches with the same token the state route
already takes, so a private document's figures are as private as its
text.

Assets are content-addressed and immutable, so the browser keeps them in
Cache Storage under the deployment's origin, the way `05-SPEC-latex.md`
keeps a distribution and `typst.wasm` is kept today. A figure is fetched
once per browser, and a re-render on a keystroke reads it from memory.

### Pruning

An asset object stays while the live `assets` map or any retained
checkpoint's tree names its digest. The retention pass that
`01-SPEC-history.md` runs at each checkpoint, which sheds the oldest
checkpoints past the ceilings, also lists the digests the surviving trees
and the live map name and deletes the rest under `assets/<slug>/`. Order
of writes: the new tree and manifest first, then the pruning, so a crash
leaves an unreferenced object and never a missing one. An object is also
kept while it is younger than `asset_grace`, because a `PUT` answers
before the client sets the map, and a checkpoint taken in that gap would
otherwise prune the digest the client is about to name. `remove` deletes
`assets/<slug>/` with everything else; `transfer` hands them over with the
document, and the bytes move from one owner's quota to the other's.

## Checkpoints

A checkpoint is named by the SHA-256 of its tree, a JSON object with sorted
keys:

```json
{
  "main": "main.tex",
  "files": {
    "chapters/03.tex": { "kind": "text", "id": "k3f9qz2m7w1p", "sha": "8b03d77…", "size": 14022 },
    "fig/one.png":     { "kind": "asset", "sha": "c41e9a0…", "size": 88123 },
    "main.tex":        { "kind": "text", "id": "a8d1x0c4nv6r", "sha": "4f2a91c…", "size": 2210 },
    "refs.bib":        { "kind": "text", "id": "p2mw7ye5hq9t", "sha": "e7710bd…", "size": 31877 }
  }
}
```

| key | contents |
| --- | --- |
| `history/<slug>/<sha>` | the tree, when the manifest entry says `"tree": true`; the source bytes of a one-file document from before, otherwise |
| `history/<slug>/blobs/<sha>` | one text, by the digest of its bytes |
| `assets/<slug>/<sha>` | one asset, by the digest of its bytes; not copied into history |

A text entry carries its id, so a restore sets the text it names rather
than a new one, and a peer editing it keeps their place.

Old checkpoints are not rewritten. A manifest entry without `tree` is
read as a tree of one text at the path `main`, with the id the migration
gave the main text and the entry's own `sha`, so the timeline shows one
continuous history across the change and a restore of an old checkpoint
is a tree of one file. The manifest entry gains `"tree": true` and
`"changed": ["chapters/03.tex", "refs.bib"]`, the paths whose digest
differs from the parent's, so the timeline can say what moved without
opening two trees. `size` is the sum over the tree, assets included,
and is what the timeline shows. It is not what the quota is measured
against: an unchanged figure appears in every tree and is stored once,
and fifty checkpoints must not charge fifty figures. The quota counts
the objects under `history/<slug>/blobs/` and `assets/<slug>/` once each,
plus the session state and the manifest. `parent`, `why`, `by`, `label`,
`commit` and `dirty` mean what they meant; `commit` and `dirty` describe
the directory's working tree, which is what they always described.

Taking a checkpoint writes the text blobs that are new, then the tree,
then the session state, then the index entry, then the manifest -- the
order `01-SPEC-history.md` gives, with the blobs in front. A blob whose
digest is already present is not written. A restore sets both maps to the
tree's contents in one transaction, with `why: restore`, and an asset a
restored tree names is still present because pruning keeps what retained
checkpoints name.

The diff view that spec plans becomes a list of changed paths with a
per-file diff behind each; a path present on one side only is shown as
added or removed whole.

## Rendering

### In the browser

`render(source, title, format)` in `renderers.js` becomes `render(tree,
title)`, where the tree is the main path, the texts as strings and the
assets as bytes. Before a compile the module calls `clear_files`, then
`add_file` for every text and every asset, then `compile` on the main
text. This is the interface the ABI has had since diagnostics; only the
caller is new. An asset not yet fetched is awaited before the first
compile that needs it, and the previous page stays up meanwhile, as it
does for any render in progress.

Typst needs nothing further. `#import`, `#include`, `#bibliography`
and `#image` all go through `World::file` and `World::source`, and
`typst-html` writes an image into the page as a base64 data URL (`rules.rs`,
`WebImage::to_base64_url`), a PDF figure among them, since `typst-svg`
turns `ImageKind::Pdf` into SVG on the way. So the rendered page carries
its figures and the frame fetches nothing. Fonts are the exception: the
engine's font book is built once from `typst_assets`, and a `.otf` in the
directory is stored and versioned but not yet offered to typst. That is
a step of its own, recorded under "Open questions".

Markdown is given a resolver from a relative `src` to a URL, and the
engine's markdown renderer gains that one argument. The URL is neither
the asset's `GET` route nor a data URL. An `<img>` cannot send the token a
private document's route takes, so the route would need the token in the
query, inside a rendered page, under an `immutable` cache; and a data
URL, which typst uses for reasons of its own, would make every
keystroke's re-render carry every figure. Instead the shell fetches each
asset once with the token, as the state route is fetched, keeps it in
Cache Storage, and hands the renderer a `blob:` URL under the frame's
origin. Typst is handed the same bytes for `add_file`. Nothing rendered
carries a token, and nothing is fetched twice.

HTML stays self-contained. A directory whose main file is `.html` is
allowed, but the identity renderer rewrites nothing in it, and
`06-SPEC-html.md`'s advice to render with resources embedded stands. A
Quarto output directory is a different and larger question and is a
non-goal here.

LaTeX's `compile(source, name)` in `05-SPEC-latex.md` becomes
`compile(tree)`: every text and asset is written into the engine's
in-memory filesystem at its path, and the engine is run on the main file.
A `.bib` in the directory is what BibTeX reads. `filecontents*` still
works and stops being the recommended way. The log's parenthesis tracking
that spec describes is what fills `file` for an error in a chapter.

### On the command line

`render_typst_document` already reads the main file's siblings within its
directory and nothing above it. Unchanged.

## In the editor

The source pane gains a file selector at its top: the paths of the
directory, main first and the rest sorted, each with the initials of the
peers whose awareness says they are in it. Beside it, one control to add a
file, which offers a name for a text or a file chooser for an asset, and
on each row the means to rename and delete it. Dropping a file on the
editor does what the add control does. Paths are shown as paths; there is
no folder tree to expand, because a paper has a dozen files and a tree is
for hundreds.

Each text gets its own CodeMirror state, created the first time it is
opened and bound with `yCollab` to its `Y.Text`; choosing a file swaps the
state, so undo history and scroll position survive a visit to another
chapter. Assets have no editor; choosing one shows it, and for a PDF the
first page.

A diagnostic whose `file` is set is listed in the badge with its path,
and choosing it opens that file at that line. Gutter marks and underlines
appear only in the file they belong to. The caret lock and the mapping
between a caret and a place in the rendered page are keyed by file as
well as line: typst spans already carry a file id, and SyncTeX names files
in both directions.

The reader pane is unchanged.

## On the command line

### `publish`

```sh
komodoc publish paper/ --title "My Paper"
komodoc publish paper/ --main chapters/../main.tex   # refused: not normalised
komodoc publish paper.tex                             # a directory of one file, as today
```

Given a directory, `publish` sends everything in it that passes the path
and extension rules, less three things: files and directories whose name
begins with `.`; the main file's own output, `<main stem>.pdf`; and, when
the directory is inside a git working tree, whatever git ignores, because
`.gitignore` is the author's own statement of what is derived; when `git`
is not on the path, nothing is ignored, and the command says so before it
sends. The main
file is `--main`, else the only text at the top level whose extension is
a document format, else `main.*`, else a refusal that lists the
candidates. Ceilings are checked before anything is sent, and a refusal
names the path.

The upload is one multipart `POST /api/documents`, the route that exists,
with one part per file named by its path; the server creates the
document, writes the texts into the session, stores the assets, fills the
maps, and takes the first checkpoint. A directory that is too large to
send in one request is a directory that is over the ceilings.

Given a file, `publish` behaves as today and publishes one file. If the
compile it runs to find the title read siblings through the file map --
the closure can say what it was asked for -- it says so:

```
paper.typ reads lib.typ and refs.bib; publish the directory to send them along
```

### `sync`

```sh
komodoc sync c9k paper/
```

The directory form of `04-SPEC-sync.md`. Every accepted file under the
directory is bound to its id, or to its path for an asset, and the watcher
covers the tree. A text that changes on disk is reconciled into its `Y.Text`
as that spec describes for one file; a text that appears is added; one that
disappears is deleted from the map, which is a deliberate act and marks
a checkpoint like a save does. An asset that changes is uploaded and its
digest set. In the other direction, a text set in the session is written
to disk, a deleted one is removed, and an asset whose digest changes is
fetched and written, with the digest-and-rename discipline the spec has
for one file. Paths are validated before anything is written under the
directory, on both sides, so a hostile peer cannot name a file outside it.

The one-file form, `komodoc sync c9k paper.tex`, is unchanged.

## Bounds

| rule | default | what it bounds |
| --- | --- | --- |
| `max_document` | 4 MB, today's `max_html` | the sum of every text and every key |
| `max_files` | 200 | keys across both maps |
| `max_assets` | 32 MB | the sum of asset objects under one document |
| `max_asset` | 8 MB | one asset |
| `asset_grace` | 1 hour | how long an unreferenced asset outlives its `PUT` |
| `max_path` | 200 bytes | one path |
| `storage.per_owner` | as today | now including assets and text blobs, each object once |
| `storage.uploads_per_hour` | as today | now including asset PUTs |

The sandbox sets `max_assets` lower than the default and its value is
measured against the R2 bill in step 3 rather than chosen here; the
sandbox's hourly expiry applies to assets as to everything else. A
self-hoster with a bucket raises it.

## What changes elsewhere

- **`01-SPEC-history.md`.** A checkpoint is a tree; text blobs move under
  `history/<slug>/blobs/`; old entries are read as one-file trees. The
  index entry gains `main`, and `size` sums the tree. The diff step
  becomes per file.
- **Diagnostics.** "In the browser it is always empty" is
  struck; `file` is filled and the editor opens it.
- **`03-SPEC-sharing.md`.** Editors upload assets; readers fetch them
  under the document's visibility; assets count against the owner's quota.
- **`04-SPEC-sync.md`.** Gains the directory form. The paragraph that
  names this work as future is resolved by it.
- **`05-SPEC-latex.md`.** `compile(source, name)` becomes `compile(tree)`.
  The non-goal "files beside the document" and the decision "a document is
  one file; `filecontents*` is how a bibliography rides along" are
  reversed by this spec; `filecontents*` keeps working.
- **`06-SPEC-html.md`.** HTML stays self-contained. The note about typst
  documents published as HTML until the project exists is resolved.
- **The Rust host and the `host/` port.** The keys above are the layout
  for both; `rules.js` carries the new rules beside `config.rs`.

## Steps

1. **The tree in the session.** `files` and `assets` maps, the migration
   of `source`, the ceiling on the sum and `max_files`, the tree
   checkpoint with `blobs/`, `tree` and `changed` in the manifest, restore
   of a tree, `main` in `meta` and mirrored on the index entry, and the
   four repairs. One-file directories only, so nothing visible changes
   -- but the wire shape changes under live tabs, which is why the
   repairs are here. Tests: migration and rollback reading, a Yrs/Yjs
   interoperability test for a map of texts, the write order under a
   crash, a restore across the old and new entry shapes, an old bundle
   writing `source` after migration and losing nothing, two concurrent
   creates at one path.
2. **Texts in the browser.** The file selector, per-file editor states,
   add, rename and delete, `file` in awareness, `clear_files` and
   `add_file` before every compile, diagnostics that open their file.
   Typst `#import` and `#bibliography` work in the browser; markdown and
   HTML are unaffected.
3. **Assets.** The two routes, `assets/<slug>/<sha>`, the quota and the
   three asset ceilings, pruning, Cache Storage, the `assets` map, typst
   images, the markdown `src` rewrite, the asset view in the source pane.
   Measure the sandbox's `max_assets`.
4. **The command line.** `publish <directory>`, the sibling hint on
   `publish <file>`, `sync <directory>`.
5. **LaTeX from a tree.** `compile(tree)` in `05-SPEC-latex.md`'s worker,
   `.bib` through BibTeX, `file` from the log. This step is done inside
   that spec's step 2 if this spec lands first, and after it otherwise.
6. **The timeline.** Changed paths on each entry, the per-file diff, and a
   download of the directory as a zip from the editor.

Steps 1 and 2 give typst authors modular documents. Step 3 gives them
figures. Step 4 gives the author with a toolchain the same. Nothing after
4 is required for the project to have directories.

## Risks

- **Two files at one path.** Two peers create `refs.bib` at the same
  moment, or two renames land on one name. Neither loses a byte, because
  texts are keyed by id; what they get is the server's suffix, a file list
  that shows both, and a rename to settle it.
- **A state that readers must download.** Every text reaches every
  reader. Bounded by `max_document`, which is the same bound a one-file
  document has today, and by the HTTP path for state above the message
  cap.
- **Assets are the cost.** A directory of figures is an order of magnitude
  larger than its text. Bounded by `max_asset`, `max_assets`, the owner's
  quota, `uploads_per_hour`, pruning to what retained checkpoints name,
  and the sandbox's expiry.
- **Paths from a hostile peer.** A key in `files` is a string any editor
  can set, and the sync client writes keys to disk. Bounded by the server
  repairing any path that fails the rules the moment it enters the
  document, and by the sync client validating again before it touches the
  filesystem.
- **Case-insensitive disks.** A directory with `Fig.png` and `fig.png` is
  refused by `publish` and suffixed by the server before it becomes a sync
  client overwriting one with the other.
- **First render waits on fetches.** A document with many figures renders
  its first page after they arrive. Bounded by the previous page staying
  up, by Cache Storage across sessions, and by in-memory reuse across
  keystrokes.
- **The retired `source` text.** A browser holding an old bundle in a tab
  across the deploy would write to a text nobody reads. Bounded by the
  server folding a write to `source` into the main text once a session is
  migrated, so the tab loses nothing and is locked out of nothing.

## Non-goals

- Discovering the tree by parsing includes.
- Compiling one chapter on its own, as `subfiles` and `standalone` allow.
  One main file; when someone asks, it is a second `main`, not a second
  model.
- A folder tree with drag and drop. Paths are shown as paths.
- HTML documents with a resource directory beside them. Self-contained,
  as today.
- Sharing an asset between documents, an asset library, a Zotero bridge.
- A git bridge. `commit` and `dirty` are provenance, not synchronisation.
- Per-file roles. Rights are per document (`03-SPEC-sharing.md`).
- Arbitrary files. The extension lists are the rule, and widening them is
  a deployment's choice.

## Decisions taken here, so they need not be reopened

- A document is a directory. The slug, the URL, the rendered page and the
  anchoring of comments do not change, and a reader is not shown a
  directory.
- Texts are a `Y.Map` of `Y.Text` in the one shared document the room
  already has, keyed by a stable id with the path as a value, so a rename
  is one set and nothing in flight is lost; there is one room per
  document, not per file.
- Assets are stored by digest under the slug and never enter the CRDT; the
  shared document holds their paths and digests only. The same bytes in
  two documents are stored twice.
- The main file is declared in the shared document and mirrored on the
  index entry. Includes are not parsed.
- A checkpoint is a tree named by its digest; text blobs live under
  `history/<slug>/blobs/`; old checkpoints are read as one-file trees and
  never rewritten.
- Kinds follow from extension lists in `config`; derived files, dotfiles
  and unnormalised paths are refused at the routes and repaired in the
  session; case-colliding paths are suffixed. Size is refused; structure
  is repaired.
- `max_document` bounds the sum of the texts; assets have their own
  ceilings and count against the owner's quota.
- `publish <file>` stays a one-file directory and says when the compile
  read siblings.
- This reverses `05-SPEC-latex.md`'s "a document is one file".
- Assets reach a renderer as bytes fetched once with the token and kept in
  Cache Storage; markdown gets a `blob:` URL, never the route and never a
  data URL.

## Open questions

- **The sandbox's `max_assets`.** Measured in step 3 against the bill,
  with the memory that cost bounding comes before every other concern.
- **Fonts for typst.** Accepting `.otf` and `.ttf` as assets is cheap;
  offering them to typst means building the font book per compile from
  the static faces plus the directory's. Deferred to a step after 3, once
  someone needs a font the engine does not ship.
- **`.gitignore` under `publish`.** Proposed honoured when the directory is
  in a git working tree, by asking git rather than parsing the file. It
  is the only way `publish` learns anything from git besides the commit.
- **A zip in, not only out.** Step 6 gives a zip out. A zip dropped on the
  landing page is the Overleaf habit and would make `publish <directory>`
  reachable from the browser; it is a small step once the routes exist,
  and it is not scheduled here.
