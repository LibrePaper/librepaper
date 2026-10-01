---
title: "Getting started"
---

Publish an HTML or Markdown document, share a link to it, and collect
comments and highlights in real time.

- Highlight passages, suggest edits, and comment on the text
- Multiple people can annotate simultaneously, with live updates
- Publish, review and manage documents in the browser
- Trivial to deploy: one static binary, on your laptop or on a small server
- Free public service for publishing and collaboration
- Share documents with signed-in GitHub and Google users
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

This installer supports Linux and macOS on x86_64 and aarch64. Windows
installers are not published for v0.0.9. To pin a version, replace
`releases/latest/download/` with `releases/download/<tag>/`.

The installer puts the executable on your PATH. It does not
add a desktop shortcut or register the `librepaper://` link handler.

## The sandbox

The public LibrePaper service is free to use. Any signed-in GitHub or Google
account can publish. Each account has 50 MiB of storage and can create or fork
projects and upload figure assets 30 times per rolling hour.

[LibrePaper](https://app.librepaper.org)

Sign in to receive private, editable tutorials for Markdown, Typst, HTML, LaTeX, and Quarto. Each tutorial contains the same LibrePaper walkthrough in that format.

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. Read, comment, and edit
links require sign-in and expire after 7 days by default. A bare document URL
does not grant access.

> **Warning:** Do not publish confidential information on the public service.
> The service operator can read all documents, and anyone you share a live link
> with can access its role. For sensitive work, [host your own instance](host.html).

The standard web-based workflow is:

1. Open a LibrePaper server in a browser,
2. Sign in with GitHub or Google,
3. Upload an `.html` or `.md` file,
4. Send a read link to readers, or mint a comment link for reviewers. Recipients
   sign in before opening the link.

Only somebody signed in and holding a live link can open the document; its bare
URL opens for you alone as the owner. The LibrePaper console lists only the
documents you own or have been let into.

Click on the thumbnails near to top of this page for screenshots of the LibrePaper management console and annotation page.
