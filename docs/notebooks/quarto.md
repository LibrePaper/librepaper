---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Two places for your files

The document is shared. The files it reads stay on your computer.

| Place | What is in it | Who sees it |
|---|---|---|
| **The LibrePaper project** | Your `.qmd`, `.bib`, images and `_quarto.yml`. | Stored on the server. Everyone you invite can read it, even readers. |
| **The Run directory** | A folder on your computer with your data, scripts and anything private. | Never uploaded. Each person picks their own. |
| **The rest of your computer** | R or Python, their packages, and the preview your code produces. | Nobody else. |

Code in a cell is part of the document. Code in a script stays in your Run directory, so share scripts with [Dropbox](#dropbox-tutorial) or [Git](#git-tutorial). A Run directory gives your code its files, not its packages, so coauthors install the same packages.

## Tutorial

1. Install the [LibrePaper desktop app](../desktop-app.html) and Quarto, plus R or Python. A web page cannot run code.
2. Open your Quarto document and press **Run code** at the top of the editor. The first time, LibrePaper asks for your Run directory: the suggested `~/LibrePaper/<project>`, or a folder you already have. Your code runs inside it. The **Run code** button's menu shows the folder and lets you change it. The Files panel shows the folder under **On this computer**, with **Open folder**. Each press runs the whole document once, and **Stop** ends a run. Nothing runs when a document opens or while you type.
3. Put your data and scripts in the Run directory. Use relative paths, such as `read.csv("data/survey.csv")`.

You can preview HTML, RevealJS slides or PDF. Word (DOCX) is download only. LibrePaper previews one document, not whole websites or books.

Someone who only writes or comments needs none of this.

## Preview and results

Between runs, the preview is the browser's draft, and no code runs. After a run to HTML, the last output sits under each code cell in the draft. It is faded when that cell's code has changed, and the **Run code** button shows how many cells have changed. Before the first run, a document with code is drawn without results.

A document with inline values, such as `` `r mean(x)` ``, shows the run's own page until your next edit. A run to PDF shows that PDF until your next edit, and the draft then has no outputs until you run again.

## Dropbox tutorial

A team shares its scripts and data through a Dropbox folder that everyone on the team has. Any sync service works the same way.

```
harvest/
├── R/
│   └── helpers.R
├── data/
│   └── survey.csv
└── figures/
    └── map.png
```

1. One person puts the scripts and data in a Dropbox folder, shares it, and makes the Quarto project in LibrePaper, inviting the team. Leave the `.qmd` out of the folder: LibrePaper already shares the document.
2. Each person opens the document, presses **Run code** and chooses their own copy of `harvest` when asked. This is done once per computer.
3. Write paths from the top of that folder, in code and in Markdown:

````markdown
```{r}
source("R/helpers.R")
survey <- read.csv("data/survey.csv")
```

![Where we sampled](figures/map.png)
````

4. Edit the scripts in any editor. Dropbox sends the changes, and each person's next run uses them.

The folder can be anywhere on each computer, so nobody writes a path to their own computer into the paper. Mark the folder as available offline, or your code may wait for a file Dropbox keeps online only.

## Git tutorial

The scripts and data are in a Git repository, and each coauthor has a clone.

1. Clone the repository, or use the clone you already have.
2. Open the document, press **Run code** and choose your clone when asked.
3. Write paths from the top of the repository, such as `source("R/helpers.R")`.
4. Change scripts, commit and push as usual. Coauthors run `git pull`. LibrePaper reads the folder as it is on disk, whatever branch is checked out.

### The paper in your own editor

To write the `.qmd` in RStudio, Positron or VS Code, connect the project to the same repository in the [Git panel](../git.html#the-git-panel) and use **Send project changes** and **Get repository changes**. Without the desktop app, `librepaper pull` and `librepaper push` do the same. Scripts never go to the project, but data files LibrePaper accepts, such as `.csv`, do, so keep private ones out with `.gitignore` or the `-librepaper` attribute (see [What syncs](../git.html#what-syncs)). LibrePaper tells you when a file differs between the project and the repository.

## Figures

| The figure | Where it is | What to expect |
|---|---|---|
| A picture you upload, such as a diagram or a logo | The LibrePaper project | Use `![](img/diagram.png)` in Markdown. Code cannot read it. |
| A file already in your Run directory | Your computer | Markdown and code can both use it. Coauthors need their own copy. |
| A plot your code draws in a cell | The preview | It appears in your preview. There is no file to manage. |

## Publish results

**Publish results** is a tick box in the **Run code** button's menu, for Quarto documents run to HTML. It is on by default, and each person sets it for each project. While it is on, each run sends the output of your last run (tables, figures, printed values) to the server, replacing the previous results. Collaborators see it under each code cell in their own browser draft, even without your data or the desktop app. Inline values are not published.

Published results count against the document owner's storage. Delete them in **Settings → Storage**, or untick **Publish results** to stop.

## Common hurdles

**Code only sees the Run directory.** Code cannot read the project's files, and LibrePaper says so when a run fails for that reason. An image that `knitr::include_graphics()` reads must be in the Run directory, or use Markdown `![](img/logo.png)` for a project image. When the same file is in both places, Markdown uses the project copy and code uses the Run directory copy, and LibrePaper tells you. Identical copies are fine.

**A file is not found.** Paths must be relative to the Run directory. Hidden files, `_quarto.yml` and folders Quarto makes (`*_files`, `*_cache`, `_freeze`) are not visible to Quarto from there.

**The `.qmd` is in a subfolder.** Paths in code still start at the top of the Run directory. Markdown paths are relative to the `.qmd`, as usual.

**Including a file your code writes.** Quarto reads `{{< include >}}` before code runs, so it works from the next run only.

**Coauthors keep data in different places.** Let someone whose data lives elsewhere override one path in their own `~/.Renviron`:

```r
harvest_root <- Sys.getenv("HARVEST_ROOT", "data/harvest_abstracts")
```

**Scripts and uploaded folders.** Scripts (`.R`, `.py`, `.jl`) cannot be uploaded. They stay in your Run directory. Scripts uploaded before this rule are hidden, but **File → Download** still includes them. Uploading a folder or zip keeps `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images and `.lua` filters, and leaves out rendered output, `_freeze`, `_site`, `_book`, `*_files`, `*_cache`, `renv` and `venv` folders, data files such as `.csv`, and scripts. To share one data file anyway, list it in `.librepaper-share.json` at the top of the project:

```json
{"include": ["data/public.csv"]}
```

**Windows network drives.** A Run directory on a network drive cannot be linked. Your code runs there, but Markdown does not see its files.

## Safety

Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit it. Code runs only when you press **Run code**. Exporting a Quarto document from the File menu also runs the code, once, when a Run directory is set. Every editor can change the code, so press **Run code** only on documents whose editors you trust.
