---
title: "Quarto"
---

[Quarto](https://quarto.org) is an open source publishing system that runs R, Python or Julia code in a Markdown document.

## Local app

The browser cannot run R, Python, or Quarto. The LibrePaper local app runs them on your computer with the tools installed there. Install [the local app](../local-app.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # check it is running and found Quarto
```

## Preview and output

Markdown preview renders the source as Markdown in the browser with front matter dropped and code chunks shown verbatim. No code runs. Quarto fenced divs such as callouts, columns, and tabsets are shown as code blocks. Unsupported shortcodes are treated the same way. This is the default for every document until you allow Quarto to run it (see Trust).

Quarto preview produces HTML, RevealJS, or PDF. DOCX is available only as an export. LibrePaper previews one document at a time. Website and book project renders are not supported.

Quarto executes locally and its generated output is not uploaded automatically. The hosted project does hold its shared source and resources. Authorized readers and commenters receive the source projection and shared assets needed for browser rendering, so keep private inputs outside the shared project. A separately published HTML file is a different document, not a privacy boundary around the source of this project.

## Trust

- Quarto runs with your user account. It can use your files, installed packages and the network. LibrePaper does not sandbox it.
- Pairing asks once per site in Settings → Render → Markdown and Quarto. Check the site and requested action before allowing it. Pairing alone runs nothing.
- Every document starts on the browser preview. Quarto runs only after you choose View > Execute code locally and accept the warning, one document at a time.
- Anyone with editor access can change that code at any time, so allow it only on documents whose owner and editors you trust.
- The permission is remembered for that document in this browser until you turn it off or sign out.

## Publishing a project

Publishing a project directory includes editorial resources and code, but skips generated output directories, execution caches, environments, and raw data by default. Add exact project-relative paths to `.librepaper-share.json` when additional inputs are intended for collaborators:

```json
{"include": ["data/public.csv"]}
```

Shared source, included inputs, and assets are readable by authorized readers as well as editors. Readers receive a projection for rendering, not CRDT history. Keep private inputs in a local companion folder and do not add them to the shared project. A project export also contains shared source and assets.

To render against a project folder on your disk instead of the hosted workspace, use binding commands. See [the companion](../cli.html#the-companion). Choosing a folder does not upload its contents. The website receives an opaque binding identifier, not the folder's absolute path.
