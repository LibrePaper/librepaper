---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Where your files live

The document is shared. The files its code reads stay on your computer.

| Place | What is in it | Who sees it |
|---|---|---|
| **The LibrePaper project** | Your `.qmd`, `.bib`, images and `_quarto.yml`. | Stored on the server. Everyone you invite can read it, even readers. |
| **The Run directory** | A folder on your computer with your data, scripts and anything private. | Never uploaded. Each person picks their own. |
| **The rest of your computer** | R or Python, their packages, and the preview your code produces. | Nobody else. |

Code in a cell is part of the document. Code in a script stays in your Run directory, so share scripts with [Dropbox](#dropbox-tutorial) or [Git](#git-tutorial). The Run directory gives your code its files, not its packages, so coauthors install the same packages.

## Tutorial

1. Install the [LibrePaper desktop app](../desktop-app.html), Quarto, and R or Python. A web page cannot run code.
2. Open your Quarto document and press **Run code** at the top of the editor. The first time, LibrePaper asks for a Run directory: the suggested `~/LibrePaper/<project>`, or a folder you already have. Your code runs there.
3. Put your data and scripts in the Run directory, and write paths relative to it, such as `read.csv("data/survey.csv")`.

A host can [turn **Run code** off](../host/advanced.html#agents-and-local-builds) for a whole server; the button is then not shown.

Each press of **Run code** runs the whole document once. **Stop** ends a run. Nothing runs when a document opens or while you type. To change the folder, open the menu beside **Run code** and choose **Run directory**, which shows the current folder's name.

You can preview HTML, RevealJS slides or PDF. Word (DOCX) is download only. LibrePaper previews one document, not whole websites or books.


## Preview and results

Between runs, the preview is the browser's draft, and no code runs. After a run to HTML, each cell's output sits under it. Output is faded once that cell's code changes, and **Run code** shows how many cells have changed.

If a run's output cannot be matched to the code cells, such as inline values like `` `r mean(x)` ``, the run's own page is shown until your next edit. A run to PDF is shown as the PDF until your next edit.

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

Nobody writes a path to their own computer into the paper. Mark the folder as available offline, or your code may wait for a file Dropbox keeps online only.

## Git tutorial

The scripts and data are in a Git repository, and each coauthor has a clone.

1. Clone the repository, or use the clone you already have.
2. Open the document, press **Run code** and choose your clone when asked.
3. Write paths from the top of the repository, such as `source("R/helpers.R")`.
4. Change scripts, commit and push as usual. Coauthors run `git pull`. LibrePaper reads the folder as it is on disk, whatever branch is checked out.

### The paper in your own editor

To write the `.qmd` in RStudio, Positron or VS Code, connect the project to the same repository in the [Git panel](../git.html#the-git-panel) and use **Send project changes** and **Get repository changes**. The `librepaper pull` and `librepaper push` commands do the same from a terminal.

Scripts never go to the project. Data files it accepts, such as `.csv`, do, so keep private ones out with `.gitignore` or the `-librepaper` attribute (see [What syncs](../git.html#what-syncs)).

## Figures

| The figure | Where it is | What to expect |
|---|---|---|
| A picture you upload, such as a diagram or a logo | The LibrePaper project | Use `![](img/diagram.png)` in Markdown. Code cannot read it. |
| A file already in your Run directory | Your computer | Markdown and code can both use it. Coauthors need their own copy. |
| A plot your code draws in a cell | The preview | It appears in your preview. There is no file to manage. |

## Publish results

**Publish results** is a tick box in the **Run code** menu. It is on by default, and each person sets it for each project. Only runs to HTML publish results.

While it is on, each run sends the output of your last run (tables, figures, printed values) to the server, replacing the previous results. Collaborators see it under each code cell in their own browser draft, even without your data or the desktop app. Inline values are not published.

Published results count against the document owner's storage. To delete them, go to **Settings → Account → Account storage**, then **Storage by document**, and choose **Delete shared results**.

## Common hurdles

**Code only sees the Run directory.** Code cannot read the project's files, and LibrePaper says so when a run fails for that reason. An image that code reads, such as one loaded with `knitr::include_graphics()`, must be in the Run directory. Markdown can use a project image with `![](img/logo.png)`. When the same path is in both places, Markdown uses the project copy and code uses the Run directory copy, and LibrePaper tells you. Identical copies are not reported.

**A file is not found.** Paths in code are relative to the top of the Run directory, not to the `.qmd`. Markdown paths are relative to the `.qmd`, as usual. Hidden files and folders, `_quarto.yml`, and folders Quarto generates (`*_files`, `*_cache`, `_freeze`, `_site`) are not linked into the project copy, so Markdown cannot use them.

**Output is stale after data changed.** With `cache: true`, Quarto reuses a cell's output until its code changes, and does not notice a data file that changed. Choose **Run without cache** in the menu beside **Run code** to recompute every cached cell once. With R (knitr), make the cache follow the file by adding `#| cache.extra: !expr tools::md5sum("data/survey.csv")` to the cell.

**Including a file your code writes.** Quarto reads `{{< include >}}` before code runs, so the included file is picked up from the next run.

**Coauthors keep data in different places.** Let someone whose data lives elsewhere override one path in their own `~/.Renviron`:

```r
harvest_root <- Sys.getenv("HARVEST_ROOT", "data/harvest_abstracts")
```

**Uploaded folders and zips.** When you upload a Quarto folder or zip, scripts (`.R`, `.py`, `.jl`) and data files such as `.csv` are left out. They stay in your Run directory. Scripts uploaded before this rule are hidden, but **File → Download** still includes them. The upload keeps `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images and `.lua` filters, and leaves out rendered output and generated folders. To share one data file anyway, list it in `.librepaper-share.json` at the top of the folder:

```json
{"include": ["data/public.csv"]}
```

**Windows network drives.** A Run directory on a network drive cannot be linked: code runs there, but Markdown does not see its files.

## Safety

Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit it. Code runs when you press **Run code**, and when you download the document as HTML, PDF or DOCX while a Run directory is set. Every editor can change the code, so press **Run code** only on documents whose editors you trust.
