---
title: "What is LibrePaper"
---

LibrePaper is a free and open source platform for collaborative writing. Write with colleagues, collect comments on a published document, or connect an LLM agent to comment and edit.

![The LibrePaper editor shows LaTeX source on the left and the compiled paper on the right, with its figure, equation and table.](../images/editor-latex.png)

## Features

The [feature table](index.html#features) lists what LibrePaper does in a web browser and in the [desktop app](desktop-app.html).

## Free and open

- Every feature is free.
- The public service at librepaper.org gives each account 100 MB of storage and 30 new projects, forks or figure uploads per hour.
- You can self-host it with Docker, as described in [Simple deployment](host/simple.html), or run `librepaper admin serve` against PostgreSQL.
- The [command line](cli.html) exports a project's source and assets as ordinary files.
- The desktop app can [back up](backups.html) every project in an account to local ZIP files.

## Chat and history

Project chat stays with the project. The owner can clear it from the chat menu or the command palette.

An owner can trim a document's history. Trimming keeps the document as it is now and deletes everything before it: editing history, named versions, chat and figures it no longer uses. Comments stay. Trimming cannot be undone.
