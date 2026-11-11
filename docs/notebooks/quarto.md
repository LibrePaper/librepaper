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
- Keep private data out of the project. Leave it on your computer and use a Run directory (below).

## Safety

- Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit what it does.
- Code runs only while **Run code** is on. It is off whenever a document opens, and LibrePaper never remembers it.
- Every editor of a document can change its code. Turn this on only for documents whose editors you trust.

## Upload a Quarto project

When you upload a project folder or zip, LibrePaper keeps the source and leaves out the rest:

- Kept: `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images, and `.lua` filters.
- Left out: rendered output, `_freeze`, `_site`, `_book`, `*_files` and `*_cache` folders, and `renv` or `venv` environments.
- Left out: data files such as `.csv`.
- Left out: `.R`, `.py` and `.jl` scripts. LibrePaper does not keep them. Keep your code in your Run directory (below).

To share a data file anyway, list it in `.librepaper-share.json` at the top of the project:

```json
{"include": ["data/public.csv"]}
```

## Run directory

Your paper lives in LibrePaper. Your code and data live in a folder on your computer: the Run directory.

- The first time you turn on **Run code** for a Quarto document, LibrePaper asks for that folder. Choose your own (for example the folder Dropbox or Git keeps in sync), or use the suggested `~/LibrePaper/<project>`.
- Each person chooses their own Run directory on their own computer. LibrePaper remembers it for this project on this computer and never uploads it.
- Next to **Run code**, the editor shows where your code runs, with **Open folder** and **Change…**.
- Your code runs in the Run directory. `read.csv("data/x.csv")` and `source("R/helpers.R")` look there.
- Markdown sees both. `![](figures/fig.png)` and `{{< include ... >}}` find files in LibrePaper and in your Run directory.
- If a file is in both places, Markdown uses LibrePaper's copy and code reads the one in your Run directory. LibrePaper tells you when this happens.
- LibrePaper never writes into your Run directory. Files your own code writes land there, as they would in RStudio.
- Not shared with Quarto: hidden files and folders, `_quarto.yml`, and folders Quarto makes itself (`*_files`, `*_cache`, `_freeze`).
- In code, refer to files in your Run directory. An image stored only in LibrePaper works in Markdown (`![](img/logo.png)`) but not in `knitr::include_graphics()`.
- A file a code chunk writes cannot be included in the same render. Quarto reads includes before code runs.
- On Windows, a Run directory on a network drive cannot be linked. Your code runs there, but Markdown does not see its files.

### Paths for coauthors

Instead of writing each person's folder into shared code, use relative paths. Let anyone whose data lives elsewhere override one path with an environment variable set in their own `~/.Renviron`:

```r
harvest_root <- Sys.getenv("HARVEST_ROOT", "data/harvest_abstracts")
```
