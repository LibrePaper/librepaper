---
title: "Getting started"
---

Publish an HTML or Markdown document, share a link to it, and collect
comments and highlights in real time.

- Highlight passages, suggest edits, and comment on the text
- Multiple people can annotate simultaneously, with live updates
- Publish, review and manage documents in the browser
- Trivial to deploy: one static binary, on your laptop or on a small server
- Free public sandbox for small, short-lived notebooks
- Allow anonymous comments or require GitHub authentication
- Export a complete, independent copy of any project from the CLI

![The annotation window, with highlights and threaded comments.](../images/commenting.png)

## Your first document

There is nothing to write to begin with. [Sign in](https://app.librepaper.org)
with GitHub or Google and your account starts with five tutorials: the same
walkthrough written in Markdown, Typst, HTML, LaTeX and Quarto, so you can read
it in whichever source language you already work in. They are yours and
private. They arrive once, when the account is made, and stay deleted if you
remove them.

Open one and drag across a sentence: a comment box opens where you released.
That is the whole of the idea; everything else in this manual is a detail of
it. The tutorial invites you to edit it, so edit it: change a sentence and
watch the preview keep up.

## Install

You do not need the CLI to read, comment on or publish a document. You need it
to run the local app, which renders Quarto and other native tools for the
browser, to export a project, or to host your own deployment.

```sh
curl -fsSL https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

On Windows, run this in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex"
```

To pin a version, replace `releases/latest/download/` in either URL with
`releases/download/<tag>/`. Older saved commands using `deploy/install.sh` still forward to the
generated installer.

The installer puts the executable on your PATH. In a new terminal, run
`librepaper local settings` to configure and start the companion. It does not
add a desktop shortcut or register the `librepaper://` link handler.

## The sandbox

The LibrePaper sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[LibrePaper sandbox](https://librepaper.arelbundock.com)

Sign in to receive private, editable tutorials for Markdown, Typst, HTML, LaTeX, and Quarto. Each tutorial contains the same LibrePaper walkthrough in that format.

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. A link that resolves to nothing gets a 404 page saying so.

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
