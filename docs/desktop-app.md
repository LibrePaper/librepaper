---
title: "The desktop app"
---

The LibrePaper desktop app runs tools on your computer. Web browsers run every page in a sandbox to protect visitors, so a website cannot run code or read your local files. The desktop app does these things for the website, under your account.

The browser keeps writing, collaborating and reading. The features below run only in the desktop app. In a browser, these features say they need the desktop app.

## What it enables

- Computational notebooks: [Quarto](notebooks/quarto.html) and [Calepin](notebooks/calepin.html) run R, Python or Julia code inside a document, so a paper, report or slide deck computes its own tables and figures. The code runs on your computer with the tools installed there, optionally against a project folder on your disk.
- [AI agents](agents.html): start the coding agent you already have and hand it the document tools.
- [Local backups](backups.html): copy every project in your account to ZIP files in a folder you pick.
- [Zotero](https://www.zotero.org): cite from your local library. Access is read-only.

Nothing runs unasked. Folders are granted one at a time through your system's own chooser. Running a document's code is approved per document. On a shared project, editors can change the code that runs, so it is their code too.

## Install

The desktop app is built from source for now.

```sh
# From a checkout of the repository
make desktop                                   # build the app and the librepaper binary it bundles
desktop/target/release/librepaper-desktop      # start the app
```

- On NixOS, `make desktop` runs inside `nix-shell`.
- `LIBREPAPER_SERVER` points the app at another server. It replaces the default server.

```sh
# Start the app with another server
LIBREPAPER_SERVER=https://paper.example.org/ desktop/target/release/librepaper-desktop
```

## Nothing else to start

The desktop app starts its own background local service when it opens. You do not start, stop or pair the local service.
To build from a checkout, see [building from source](architecture/building.html).
