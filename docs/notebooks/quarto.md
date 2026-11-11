---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Two places for your files

| Place | What is in it | Who sees it |
|---|---|---|
| **The LibrePaper project** | Your `.qmd`, `.bib`, images and `_quarto.yml`. | Stored on the server. Everyone you invite can read it, even readers. It has no access to your computer. |
| **The Run directory** | A folder on your computer with your data, scripts and anything private. | Never uploaded. Each person picks their own, on their own computer. |

When you run code, Quarto sees both. Your code runs inside the Run directory.

## Tutorial

1. Install the [LibrePaper desktop app](../desktop-app.html) and Quarto, plus R or Python. The app finds Quarto on your computer. A web page cannot run code.
2. Open your Quarto document and turn on **Run code** at the top of the editor. It is off each time a document opens.
3. The first time, LibrePaper asks for your Run directory. Use the suggested `~/LibrePaper/<project>`, or choose your own folder, for example one that Dropbox or Git keeps in sync.
4. Put your data and scripts there. Use relative paths, such as `read.csv("data/survey.csv")` and `source("R/helpers.R")`.
5. Edit. The preview renders on your computer. Next to **Run code**, you see where your code runs, with **Open folder** and **Change…**.

- Your output stays on your computer.
- You can preview HTML, RevealJS slides or PDF. Word (DOCX) is download only.
- LibrePaper previews one document. It does not render whole websites or books.

## Common hurdles

**A file is not found.** Code looks in your Run directory. Check that the file is there and that the path is relative to it.

**The same file is in both places.** Markdown uses the LibrePaper copy. Code reads the one in your Run directory. LibrePaper tells you when this happens. Identical copies are fine.

**An image works in Markdown but not in `knitr::include_graphics()`.** Code only sees the Run directory. Keep that image there, or use Markdown `![](img/logo.png)`.

**Including a file your code writes.** Quarto reads `{{< include >}}` before code runs, so a file made in the same render cannot be included. It works from the next render.

**Coauthors keep data in different places.** Do not write anyone's folder into shared code. Use relative paths. Let someone whose data lives elsewhere override one path in their own `~/.Renviron`:

```r
harvest_root <- Sys.getenv("HARVEST_ROOT", "data/harvest_abstracts")
```

**Scripts (`.R`, `.py`, `.jl`) cannot be uploaded.** LibrePaper keeps the paper, not the code. Put scripts in your Run directory. Scripts uploaded before this rule are not shown in the project, but downloading the project still includes them.

**What uploading a folder or zip keeps.** Kept: `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images and `.lua` filters. Left out: rendered output, `_freeze`, `_site`, `_book`, `*_files`, `*_cache`, `renv` and `venv` folders, data files such as `.csv`, and scripts. To share one data file anyway, list it in `.librepaper-share.json` at the top of the project:

```json
{"include": ["data/public.csv"]}
```

**Not visible to Quarto from the Run directory:** hidden files and folders, `_quarto.yml`, and folders Quarto makes (`*_files`, `*_cache`, `_freeze`).

**Windows network drives.** A Run directory on a network drive cannot be linked. Your code runs there, but Markdown does not see its files.

## Safety

- Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit it.
- Code runs only while **Run code** is on. LibrePaper never remembers this setting.
- Every editor can change the code. Turn on **Run code** only for documents whose editors you trust.
