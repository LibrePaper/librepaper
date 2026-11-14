---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Two places for your files

The document is shared. The files it reads stay on your computer.

| Place | What is in it | Who sees it |
|---|---|---|
| **The LibrePaper project** | Your `.qmd`, `.bib`, images and `_quarto.yml`. | Stored on the server. Everyone you invite can read it, even readers. It has no access to your computer. |
| **The Run directory** | A folder on your computer with your data, scripts and anything private. | Never uploaded. Each person picks their own. |
| **The rest of your computer** | R or Python, their packages, and the preview your code produces. | Nobody else. Each person installs their own tools. |

Code in a cell is part of the document, so everyone edits it together. Code in a script is a file the document reads, so it stays in your Run directory. Share scripts with [Dropbox](#dropbox-tutorial) or [Git](#git-tutorial).

A Run directory gives your code its files, not its packages. A coauthor needs the same packages to get the same result.

## Tutorial

1. Install the [LibrePaper desktop app](../desktop-app.html) and Quarto, plus R or Python. The app finds Quarto on your computer. A web page cannot run code.
2. Open your Quarto document and press **Run code** at the top of the editor. Each press runs the whole document once, from the top. **Stop** ends a run. Nothing runs when a document opens or while you type.
3. The first time, LibrePaper asks for your Run directory. Use the suggested `~/LibrePaper/<project>`, or choose a folder you already have. The **Run code** button's menu shows the folder and lets you change it. Quarto code runs inside it.
4. Put your data and scripts there. Use relative paths, such as `read.csv("data/survey.csv")` and `source("R/helpers.R")`.
5. Edit, then press **Run code** again. The Files panel shows the same folder under **On this computer**, with **Open folder**.

Your output stays on your computer. You can preview HTML, RevealJS slides or PDF. Word (DOCX) is download only. LibrePaper previews one document, not whole websites or books.

Someone who only writes or comments needs none of this. They see the document, and any results you publish.

## Preview and results

Between runs, the preview is the browser's draft of the document. No code runs. Before the first run, a document with code is drawn without results.

After a run to HTML, the output of your last run appears under each code cell in the draft, so prose edits show at once. Output from a cell whose code has changed since the run is faded until the next run. The **Run code** button shows how many cells have changed.

Two cases differ. A document with inline values, such as `` `r mean(x)` ``, shows the run's own page until your next edit, since inline values have no cell to sit under. A run to PDF shows the PDF that run made until your next edit. After that the draft has no outputs until you run again.

## Dropbox tutorial

A team writes one paper. Its scripts and data are in a Dropbox folder that everyone on the team has. Any other sync service works the same way.

```
harvest/
├── R/
│   └── helpers.R
├── data/
│   └── survey.csv
└── figures/
    └── map.png
```

1. One person makes the folder in Dropbox, puts the scripts and data in it, and shares it with the team. Leave the `.qmd` out: LibrePaper already shares the document.
2. One person makes the Quarto project in LibrePaper and invites the team.
3. Each person opens the document in the desktop app, presses **Run code** and chooses their own copy of `harvest` when asked. This is done once per computer.
4. Write paths from the top of that folder, in code and in Markdown:

````markdown
```{r}
source("R/helpers.R")
survey <- read.csv("data/survey.csv")
```

![Where we sampled](figures/map.png)
````

5. Edit the scripts in any editor. Dropbox sends the changes, and each person's next run uses them.

The folder can be anywhere. One person has `~/Dropbox/harvest` and another has `D:\Dropbox\harvest`, and the same document runs for both. Nobody writes a path to their own computer into the paper. If Dropbox keeps a file online only, your code may wait for the download or fail. Mark the folder as available offline.

## Git tutorial

The scripts and data are in a Git repository, and each coauthor has a clone.

1. Clone the repository, or use the clone you already have.
2. Open the document in the desktop app, press **Run code** and choose your clone when asked.
3. Write paths from the top of the repository, such as `source("R/helpers.R")`.
4. Change scripts on a branch, commit and push as usual. Coauthors run `git pull`, and their next run uses the new scripts. LibrePaper reads the folder as it is on disk, whatever branch is checked out.

That is all a team needs when the paper is written in LibrePaper. To write the `.qmd` in RStudio, Positron or VS Code instead, connect the project to the same repository:

1. In the desktop app, open the [Git panel](../git.html#the-git-panel), click **Browse…** and choose your clone, then click **Connect**.
2. Click **Send project changes**. The `.qmd`, the bibliography and the images arrive in the repository.
3. Edit and commit in your editor. Click **Get repository changes**, and your commits appear in the project as named versions under your name.

Changes move when you click, not keystroke by keystroke. Without the desktop app, `librepaper pull` and `librepaper push` do the same from a terminal.

Scripts never go to the project. Data files of a kind LibrePaper accepts, such as `.csv`, do go with everything else, and every collaborator can read them. Keep private ones out with `.gitignore` or the `-librepaper` attribute. See [What syncs](../git.html#what-syncs). While a file differs between the project and the repository, LibrePaper tells you when you run code. Sync to make them agree.

## Figures

| The figure | Where it is | What to expect |
|---|---|---|
| A picture you upload, such as a diagram or a logo | The LibrePaper project | Use it in Markdown: `![](img/diagram.png)`. Everyone sees it. Code cannot read it. |
| A file already in your Run directory | Your computer | Markdown and code can both use it. Coauthors need their own copy, through Dropbox or Git. |
| A plot your code draws in a cell | The preview, on your computer | It appears in your preview. There is no file to manage. |

## Publish results

**Publish results** works for Quarto documents run to HTML. It is a tick box in the **Run code** button's menu, on by default. Each person sets it for each project.

While it is on, each run sends the output of your last run (tables, figures, printed values) to the server. Collaborators see it under each code cell in their own browser draft, even without your data or the desktop app. Each run replaces the published results. Inline values are not published.

Published results count against the storage of the document's owner. To delete them, go to **Settings → Storage**. To stop, untick **Publish results**.

## Common hurdles

**A file is not found.** Code looks in your Run directory, and paths must be relative to it. Hidden files, `_quarto.yml` and folders Quarto makes (`*_files`, `*_cache`, `_freeze`) are not visible to Quarto from there.

**Code cannot read a file you uploaded to LibrePaper.** LibrePaper tells you when this is why a run failed. Keep data in your Run directory.

**An image works in Markdown but not in `knitr::include_graphics()`.** Code only sees the Run directory. Keep that image there, or use Markdown `![](img/logo.png)`.

**The same file is in both places.** Markdown uses the LibrePaper copy, and code reads the one in your Run directory. LibrePaper tells you when this happens. Identical copies are fine.

**The `.qmd` is in a subfolder.** Paths in code still start at the top of the Run directory. Markdown paths are relative to the `.qmd`, as usual.

**Including a file your code writes.** Quarto reads `{{< include >}}` before code runs, so a file made in the same run cannot be included. It works from the next run.

**Coauthors keep data in different places.** Do not write anyone's folder into shared code. Use relative paths. Someone whose data lives elsewhere can override one path in their own `~/.Renviron`:

```r
harvest_root <- Sys.getenv("HARVEST_ROOT", "data/harvest_abstracts")
```

**Scripts (`.R`, `.py`, `.jl`) cannot be uploaded.** A script stays on your computer, in your Run directory. Scripts uploaded before this rule are not shown in the project, but downloading the project (**File → Download**) still includes them.

**What uploading a folder or zip keeps.** Kept: `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images and `.lua` filters. Left out: rendered output, `_freeze`, `_site`, `_book`, `*_files`, `*_cache`, `renv` and `venv` folders, data files such as `.csv`, and scripts. To share one data file anyway, list it in `.librepaper-share.json` at the top of the project:

```json
{"include": ["data/public.csv"]}
```

**Windows network drives.** A Run directory on a network drive cannot be linked. Your code runs there, but Markdown does not see its files.

## Safety

Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit it. Code runs only when you press **Run code**. Exporting a Quarto document from the File menu also runs the code, once, when a Run directory is set. Every editor can change the code, so press **Run code** only on documents whose editors you trust.
