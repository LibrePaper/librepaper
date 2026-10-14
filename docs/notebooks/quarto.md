---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Set up

A web page cannot run your code. The LibrePaper companion is a small app that runs Quarto on your own computer, with the R, Python and packages you already have. Install [the companion](../local-app.html), then start it:

```sh
librepaper                           # start it
librepaper --at-login                # start it now and every time you log in
librepaper status                    # check that it found Quarto
```

## Preview

- A new document shows a plain Markdown preview. No code runs. Code shows as text, and Quarto extras such as callouts, columns and tabs show as raw text too.
- To see the real Quarto output, choose **View → Execute code locally** and accept the warning. Quarto then renders on your computer.
- Quarto can show HTML, slides (RevealJS) or PDF. Word (DOCX) is a download only.
- LibrePaper previews one document. It does not render whole Quarto websites or books.

## Who sees what

- The output Quarto makes stays on your computer. LibrePaper does not upload it.
- The source files in the project are stored on the server. Anyone you share the project with can read them, even readers.
- Keep private data out of the project. Leave it on your computer and use a project folder (below).

## Safety

- Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit what it does.
- Code runs only in documents where you chose **Execute code locally**. The choice is per document and per browser. Signing out turns it off everywhere.
- Every editor of a document can change its code. Turn this on only for documents whose editors you trust.

## Upload a Quarto project

When you upload a project folder or zip, LibrePaper keeps the source and leaves out the rest:

- Kept: `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images, and `.R`, `.py`, `.jl` and `.lua` scripts.
- Left out: rendered output, `_freeze`, `_site`, `_book`, `*_files` and `*_cache` folders, and `renv` or `venv` environments.
- Left out: data files such as `.csv`.

To share a data file anyway, list it in `.librepaper-share.json` at the top of the project:

```json
{"include": ["data/public.csv"]}
```

## Use a folder on your computer

You can render against a folder on your computer instead of the copy on the server. This is how you use private data. Choose **File → Project folder…** and pick the folder. LibrePaper never uploads that folder and never learns its path.
