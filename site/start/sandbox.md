---
title: "The sandbox"
---

The LibrePaper sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[LibrePaper sandbox](https://librepaper.arelbundock.com)

Sign in to receive private, editable tutorials for Markdown, Typst, HTML, LaTeX, and Quarto. Each tutorial contains the same LibrePaper walkthrough in that format.

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. Curated tutorial documents
are the exception: their suffix is derived from the title rather than drawn at
random, so re-seeding the sandbox leaves these links pointing at the same
documents. That is safe only because an example is public on purpose; every
other document keeps an unguessable address. A link that resolves to nothing
gets a 404 page saying so.

> **Warning:** Do not publish confidential information on the LibrePaper sandbox. Normally, documents are only visible to the person who uploaded them, or to people holding a share link they minted. But if you are gathering comments on documents about national security, you should probably [host your own instance](../host/index.md) or find another solution.

The standard web-based workflow is:

1. Open a LibrePaper server in a browser,
2. Sign in with GitHub (if the manager requires it),
3. Upload an `.html` or `.md` file,
4. Send the read link to your readers, or mint a comment link and send that.

Only somebody holding a live link can open the document; its bare URL opens
for you alone. The LibrePaper console lists only the documents you own or have
been let into.

Click on the thumbnails near to top of this page for screenshots of the LibrePaper management console and annotation page.
