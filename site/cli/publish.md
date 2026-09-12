---
title: "Publishing and syncing"
---

## Publish

Publish an HTML, Markdown, or Quarto document:

```sh
librepaper publish paper.html --title "My Paper"
```

An HTML file is accepted as source and may be self-contained, with images,
styles, and fonts embedded. For a Quarto project, publish `paper.qmd` and its
declared input files instead of uploading a generated HTML result. This
preserves the source and does not run its code. See [Quarto documents](../authoring/quarto.md)
for browser preview and local rendering.

Publishing a file again, to a document that already exists, writes the file's
text into the live document and marks a checkpoint in its history. It never
conflicts with someone editing in the browser: their words and yours end up in
the same document, the way two browsers' do.

### A paper is usually several files

A document is a directory, so publish the directory:

```sh
librepaper publish paper/ --title "My Paper"
```

Everything in it goes: the chapters, the `.bib`, the figures. Three things are
left behind: names beginning with a dot, the main file's own `.pdf`, and
whatever git ignores, since a `.gitignore` is the author's own statement of
what is derived. Which file is the document is the one text at the top level
that LibrePaper renders, or `main.*`; when neither settles it, `--main` does:

```sh
librepaper publish paper/ --main chapters/thesis.typ
```

Quarto projects use the narrower [sharing policy](../authoring/quarto.md),
which also excludes raw data, execution caches, and generated output by default.

Publishing a single file that reads its neighbours says so rather than
publishing a project that compiles here and nowhere else. An editor opening
the project would otherwise be unable to reproduce its rendering:

```
paper.typ reads lib.typ and refs.bib; publish the directory to send them along:
  librepaper publish .
```

## Sync

The editor in the page is one door into a live session. `sync` is the other:
it makes the file on your disk a peer in the same session, so you can work in
vim, Positron or Emacs and still be in the document everyone else is reading.

```sh
librepaper sync c9k paper.typ
```

```
syncing paper.typ with https://librepaper.example.org/docs/coverage-t2rpf5rzq6
joined the session (2 peers)
paper.typ changed
checkpoint 4f2a91c
session changed: wrote paper.typ
```

It runs until you stop it. Edits you save flow into the session while a browser
tab is typing in it; edits made in the browser land in your file, written
atomically so your editor never reads half of one. Saving is a deliberate act,
so it asks for a checkpoint in the timeline -- a burst of saves is one mark,
and saving text the document already has is none. If the file is not there
when you start, it is written from the document, which is how you pull one
down to edit locally.

For Quarto source documents, synchronize `paper.qmd` and its declared inputs.
Synchronizing `paper.html` is supported only when that HTML is deliberately
published as the document's source; generated preview output is not retained.

**The one thing worth understanding.** Your editor is a snapshot client: it
read the file at some moment and writes its whole buffer back when you save.
If the session moved in between -- you in a browser tab, a coauthor, a restore
-- your buffer knows nothing about it. So a save is merged rather than
applied: edits to different paragraphs all go through, and where both sides
changed the same words the session wins, your file is brought forward, and the
terminal says which words it gave up. The merge is by word, not by line,
because a paragraph here is one line.

It is not live collaboration from vim: your side of the session moves when the
file is written. Turn on your editor's auto-save if you want it to move often.
Two people typing in the same paragraph at the same time is what the browser
editor is for.

One file, one document, one session. `--interval` (default `250ms`) sets how
long either side stays quiet before it is acted on. A lock beside the file
stops you running two of these on it by accident. Turn off format-on-save for
a synced file: a formatter that rewrites every line is a change against
everyone and re-anchors every comment.
