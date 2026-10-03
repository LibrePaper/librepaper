---
title: "Quarto"
---

Quarto is an open source publishing system that runs R, Python or Julia code in a Markdown document.

## Local app

The browser cannot run R, Python, or Quarto; the LibrePaper local app runs them on your computer with the tools installed there. Install from the [install page](../install.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # check it is running and found Quarto
```

When the server runs on the same machine you browse from, it runs the local app itself and there is nothing to start. Use `--no-local` to turn that off. For a computer with no display, run `librepaper local approve`.

> **Warning:** Previews run the document's code, filters and scripts on your computer, so only enable this on documents you trust. Anyone with editor access can change that code.

## Preview and output

Markdown preview renders the source as Markdown in the browser with front matter dropped and code chunks shown verbatim. No code runs. Quarto fenced divs such as callouts, columns, and tabsets are shown as code blocks; unsupported shortcodes are treated the same way. This is the fallback when no local app is paired.

Quarto preview produces HTML, RevealJS, or PDF. DOCX is available only as an export. LibrePaper previews one document at a time; website and book project renders are not supported.

Nothing rendered is ever uploaded. The server holds only the `.qmd` source and its declared shared resources.

## Sandbox

Executable previews and renders require operating-system confinement: Bubblewrap on Linux or `sandbox-exec` on macOS. LibrePaper refuses to run Quarto code when confinement is unavailable. The renderer can see the connected project or synchronized preview workspace, but paths outside that scope are hidden and network access is disabled. For an isolated one-shot render, additional local data must be named explicitly as a project-relative input; only those inputs are copied into the temporary workspace.

## Publishing a project

Publishing a project directory includes editorial resources and code, but skips generated output directories, execution caches, environments, and raw data by default. Add exact project-relative paths to `.librepaper-share.json` when additional inputs are intended for collaborators:

```json
{"include": ["data/public.csv"]}
```

Shared source and inputs are readable by owners and editors. Readers and commenters receive only explicitly published HTML and its display assets.

To render against a project folder on your disk instead of the hosted workspace, use binding commands; see [the companion](../cli.html#the-companion). Choosing a folder does not upload its contents. The website receives an opaque binding identifier, not the folder's absolute path.
