---
title: "Getting started"
---

Publish an HTML or Markdown document, share a link to it, and collect
comments and highlights in real time.

- Highlight passages, suggest edits, and comment on the text
- Multiple people can annotate simultaneously, with live updates
- Publish documents from the CLI; review and manage them in the browser
- Trivial to deploy: one static binary, on your laptop or on a small server
- Free public sandbox for small, short-lived notebooks
- Allow anonymous comments or require GitHub authentication
- Export annotations as Markdown or W3C JSON-LD

![The annotation window, with highlights and threaded comments.](../images/commenting.png)

## Your first document

There is nothing to write to begin with. The tutorial is a real LibrePaper
document, already published and already carrying a few comments, and it is
open to anyone:

**[Learn LibrePaper with Markdown](https://app.librepaper.org/docs/learn-librepaper-with-markdown-t678fbd47v)**

Drag across a sentence and a comment box opens where you released. That is the
whole of the idea; everything else in this manual is a detail of it.

The tutorial invites you to edit it, so edit it: change a sentence and watch
the preview keep up.

## The same document, in your own account

Signing in gives you your own private copy of that tutorial, and four more
beside it — the same walkthrough written in Typst, HTML, LaTeX and Quarto, so
you can read it in whichever source language you already work in. They arrive
once, when the account is made, and stay deleted if you remove them.

| Format | Tutorial |
| --- | --- |
| Markdown | [Learn LibrePaper with Markdown](https://app.librepaper.org/docs/learn-librepaper-with-markdown-t678fbd47v) |
| Typst | [Learn LibrePaper with Typst](https://app.librepaper.org/docs/learn-librepaper-with-typst-8fzxwcqcnc) |
| HTML | [Learn LibrePaper with HTML](https://app.librepaper.org/docs/learn-librepaper-with-html-8vqyakbbyb) |
| LaTeX | [Learn LibrePaper with LaTeX](https://app.librepaper.org/docs/learn-librepaper-with-latex-ua3e2x26cw) |
| Quarto | [Learn LibrePaper with Quarto](https://app.librepaper.org/docs/learn-librepaper-with-quarto-mgprkwz4m7) |

Those addresses do not change. A tutorial's suffix is derived from its
title rather than drawn at random, so every account receives the same links —
which is safe only because a tutorial is public on purpose. Every other document
keeps an unguessable address. See [the sandbox](#the-sandbox) for what that
means for your own work.

## Install

You do not need the CLI to read or comment on a document. You need it to
publish one from a project on your own machine.

```sh
curl -fsSL https://raw.githubusercontent.com/LibrePaper/librepaper/main/deploy/install.sh | sh
```

The installer supports Linux and macOS. Windows binaries are available on the
[releases page](https://github.com/LibrePaper/librepaper/releases).

Then [publish something of your own](cli.html#publish).

## The sandbox

The LibrePaper sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[LibrePaper sandbox](https://librepaper.arelbundock.com)

Sign in to receive private, editable tutorials for Markdown, Typst, HTML, LaTeX, and Quarto. Each tutorial contains the same LibrePaper walkthrough in that format.

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. Tutorial documents
are the exception: their suffix is derived from the title rather than drawn at
random, so every new account receives the same links. That is safe only because
a tutorial is public on purpose; every other document keeps an unguessable
address. A link that resolves to nothing gets a 404 page saying so.

> **Warning:** Do not publish confidential information on the LibrePaper sandbox. Normally, documents are only visible to the person who uploaded them, or to people holding a share link they minted. But if you are gathering comments on documents about national security, you should probably [host your own instance](host.html) or find another solution.

The standard web-based workflow is:

1. Open a LibrePaper server in a browser,
2. Sign in with GitHub (if the manager requires it),
3. Upload an `.html` or `.md` file,
4. Send the read link to your readers, or mint a comment link and send that.

Only somebody holding a live link can open the document; its bare URL opens
for you alone. The LibrePaper console lists only the documents you own or have
been let into.

Click on the thumbnails near to top of this page for screenshots of the LibrePaper management console and annotation page.
