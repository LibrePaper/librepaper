---
title: "Quarto"
---

## Quarto documents

Publish and synchronize the actual `.qmd` source rather than a rendered
result; see [the CLI](../cli.html).

The Tools menu offers exactly one active preview mode at a time, remembered
per document (default: **Quarto preview**):

- **Markdown preview** renders the source as Markdown in the browser — front
  matter dropped and code chunks shown verbatim — and never runs code.
  Quarto fenced divs such as callouts, columns, and tabsets are shown as
  whitespace-preserving code blocks rather than partially interpreted HTML;
  unsupported shortcodes are treated the same way.
- **Quarto preview** runs the document with Quarto on your own computer,
  through the local app. Document previews may produce HTML, RevealJS, or
  PDF; DOCX is available only as an export.

Rendering runs on your own computer, with Quarto and R or Python installed
there. Enable local rendering from the preview banner, or start the companion from a
terminal; see [the companion](../cli.html#the-companion).

The first time you pick Quarto preview, a small window from the local app
asks whether to allow that site to use this computer's tools; click
**Allow**. Once paired, your edits sync into the app's own workspace, Quarto
re-renders there, and the pane polls the result and paints it in, so comments
and highlights work on the live page too. The previous render stays on screen
while a new one is under way, so figures never flash blank. If the browser
isn't paired, or no local app is available, the pane shows Markdown preview
with a **Connect** button in the banner. These run the document's code,
filters, and scripts on your machine, so allow only sites you trust. The
pairing code the app prints still works as a fallback under Tools, **Local
app settings…**. Nothing rendered is ever uploaded: the server holds only the
`.qmd` source and its declared shared resources. Website and book project
renders are not supported; LibrePaper previews one document at a time.

Executable previews and renders require operating-system confinement:
Bubblewrap on Linux or `sandbox-exec` on macOS. LibrePaper refuses to run
Quarto code when confinement is unavailable. The renderer can see the
connected project or synchronized preview workspace, but paths outside that
scope are hidden and network access is disabled. For an isolated one-shot
render, additional local data must be named explicitly as a project-relative
input; only those inputs are copied into the temporary workspace.

When the server runs on the machine you browse from, it runs the local app
itself and there is nothing to start. `--no-local` turns that off.

Publishing a project directory includes editorial resources and code, but skips
generated output directories, execution caches, environments, and raw data by
default. Add exact project-relative paths to `.librepaper-share.json` when an
additional input is intended for collaborators:

```json
{"include": ["data/public.csv"]}
```

Review the source-upload inventory. Shared source and inputs are readable by
owners and editors. Readers and commenters receive only explicitly published
HTML and its display assets. Files needed only for local execution can
remain in the author's project.

A project that keeps data the document does not share can be linked with
**Choose project folder…** in the local rendering settings. The native folder
dialog runs on this computer, and the selection is remembered for this site
and document. Choosing a folder does not upload its contents. The website
receives an opaque binding identifier, not the folder's absolute path.

Terminal users can make the same explicit link, which takes precedence for
render jobs for the document:

The paired app can render against a project folder on your disk instead of its
hosted workspace for this document. Binding and revoking are terminal
commands; see [the companion](../cli.html#the-companion).
