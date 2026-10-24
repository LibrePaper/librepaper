---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Set up

A web page cannot run your code. The [LibrePaper desktop app](../desktop-app.html) runs Quarto on your own computer, with the R, Python and packages you already have. Install the app. The app finds Quarto when it is installed on your computer.

## Preview

- A new document shows a plain Markdown preview. No code runs. Code shows as text, and Quarto extras such as callouts, columns and tabs show as raw text too.
- To see the real Quarto output, turn on **Run code** at the top of the editor. Quarto then renders on your computer.
- Quarto can show HTML, slides (RevealJS) or PDF. Word (DOCX) is a download only.
- LibrePaper previews one document. It does not render whole Quarto websites or books.

## Who sees what

- The output Quarto makes stays on your computer. LibrePaper does not upload it.
- The source files in the project are stored on the server. Anyone you share the project with can read them, even readers.
- Keep private data out of the project. Leave it on your computer and use a project folder (below).

## Safety

- Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit what it does.
- Code runs only while **Run code** is on. It is off whenever a document opens, and LibrePaper never remembers it.
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

You can render with a folder on your computer. This is how you use private data, packages and caches without uploading them.

- Open the desktop app.
- Choose **File → Project folder…**.
- Check the name of the file you preview.
- Click **Choose folder…** and approve.

Before each render, LibrePaper writes the project's files into that folder. The folder always matches the editor.

- LibrePaper never touches other files there, such as data or environments.
- LibrePaper never deletes anything.
- If you changed one of the project's files in that folder yourself, LibrePaper keeps your version as `<name>.local` before it writes. For example, `paper.qmd.local`.

LibrePaper never uploads the folder and never learns its path.
