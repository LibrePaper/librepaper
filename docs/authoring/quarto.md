---
title: "Quarto"
---

## Quarto documents

Publish and synchronize the actual `.qmd` source rather than a rendered
result; see [the CLI](../cli.html).

Preview modes (default: Quarto preview):

- **Markdown preview** renders the source as Markdown in the browser, with
  front matter dropped and code chunks shown verbatim. No code runs.
  Quarto fenced divs such as callouts, columns, and tabsets are shown as
  code blocks; unsupported shortcodes are treated the same way.
- **Quarto preview** runs the document with Quarto on your own computer,
  through the local app. Previews may produce HTML, RevealJS, or PDF; DOCX
  is available only as an export.

## Install and start the local app

Quarto preview runs on your own computer, with Quarto and R or Python
installed there. The LibrePaper local app does the rendering. Install it
from the [install page](../install.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
librepaper status                    # check it is running and found Quarto
```

When the server runs on the machine you browse from, it runs the local app
itself and there is nothing to start. `--no-local` turns that off.

More commands are in [the CLI reference](../cli.html#the-companion).

## Connect the browser

When you select Quarto preview, the browser asks the local app to connect.
Allow it in the dialog the app shows (on a computer with no display, run
`librepaper local approve`). Once paired, your edits sync into the app's
workspace, Quarto re-renders there, and the preview updates with comments and
highlights working on the live page. Previews run the document's code, filters,
and scripts on your machine, so allow only sites you trust. Nothing rendered
is ever uploaded: the server holds only the `.qmd` source and its declared
shared resources. Website and book project renders are not supported; one
document at a time.

## Sandbox

Executable previews and renders require operating-system confinement:
Bubblewrap on Linux or `sandbox-exec` on macOS. LibrePaper refuses to run
Quarto code when confinement is unavailable. The renderer can see the
connected project or synchronized preview workspace, but paths outside that
scope are hidden and network access is disabled. For an isolated one-shot
render, additional local data must be named explicitly as a project-relative
input; only those inputs are copied into the temporary workspace.

## Publishing a project

Publishing a project directory includes editorial resources and code, but
skips generated output directories, execution caches, environments, and raw
data by default. Add exact project-relative paths to `.librepaper-share.json`
when additional inputs are intended for collaborators:

```json
{"include": ["data/public.csv"]}
```

Shared source and inputs are readable by owners and editors. Readers and
commenters receive only explicitly published HTML and its display assets.

To render against a project folder on your disk instead of the hosted
workspace, use binding commands; see [the companion](../cli.html#the-companion).
Choosing a folder does not upload its contents. The website receives an opaque
binding identifier, not the folder's absolute path.
