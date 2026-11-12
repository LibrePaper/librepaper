---
title: "Quarto"
---

[Quarto](https://quarto.org) turns a Markdown file with R, Python or Julia code into a report, slides or a PDF.

## Two places for your files

The document is shared. The files it reads stay on your computer.

| Place | What is in it | Who sees it |
|---|---|---|
| **The LibrePaper project** | Your `.qmd`, `.bib`, images and `_quarto.yml`. | Stored on the server. Everyone you invite can read it, even readers. It has no access to your computer. |
| **The Run directory** | A folder on your computer with your data, scripts and anything private. | Never uploaded. Each person picks their own, on their own computer. |
| **The rest of your computer** | R or Python, their packages, and the preview your code produces. | Nobody else. Each person installs their own tools and renders their own preview. |

When you run code, Quarto sees both. Your code runs inside the Run directory.

Code in a cell is part of the document, so everyone edits it together. Code in a script is a file the document reads, so it stays in your Run directory. Share scripts the way you share data, with [Dropbox](#dropbox-tutorial) or [Git](#git-tutorial).

A Run directory gives your code its files. It does not install packages, so a coauthor needs the same ones to get the same result.

## Tutorial

Write together in LibrePaper. Run computations on your computer. Choose a local folder with your scripts and data.

1. Install the [LibrePaper desktop app](../desktop-app.html) and Quarto, plus R or Python. The app finds Quarto on your computer. A web page cannot run code.
2. Open your Quarto document and turn on **Run code** at the top of the editor. It is off each time a document opens.
3. The first time, LibrePaper asks for your Run directory. Use the suggested `~/LibrePaper/<project>`, or choose a folder you already have.
4. Put your data and scripts there. Use relative paths, such as `read.csv("data/survey.csv")` and `source("R/helpers.R")`.
5. Edit. The preview renders on your computer. The folder name next to **Run code** shows where your code runs; click it to change the folder. The Files panel shows the same folder under **On this computer**, with **Open folder**.

Your output stays on your computer. You can preview HTML, RevealJS slides or PDF; Word (DOCX) is download only. LibrePaper previews one document, not whole websites or books.

Someone who only writes or comments needs none of this. They work in the browser and see the document without its results.

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

1. One person makes the folder in Dropbox, puts the scripts and data in it, and shares it with the team. Leave the `.qmd` out of it: LibrePaper already shares the document.
2. One person makes the Quarto project in LibrePaper and invites the team.
3. Each person opens the document in the desktop app, turns on **Run code**, clicks **Choose another folder** and picks their own copy of `harvest`. This is done once per computer.
4. Write paths from the top of that folder, in code and in Markdown:

````markdown
```{r}
source("R/helpers.R")
survey <- read.csv("data/survey.csv")
```

![Where we sampled](figures/map.png)
````

5. Edit the scripts in any editor. Dropbox sends your changes to the team, and each person's next render uses them.

The folder can be anywhere. One person has `~/Dropbox/harvest` and another has `D:\Dropbox\harvest`, and the same document runs for both. Nobody writes a path to their own computer into the paper.

If Dropbox keeps a file online only, your code may wait for the download or fail to read it. Mark the folder as available offline.

## Git tutorial

The scripts and data are in a Git repository, and each coauthor has a clone.

1. Clone the repository, or use the clone you already have.
2. Open the document in the desktop app, turn on **Run code**, click **Choose another folder** and pick your clone.
3. Write paths from the top of the repository, such as `source("R/helpers.R")`.
4. Change scripts on a branch, commit and push as usual. Coauthors run `git pull`, and their next render uses the new scripts. LibrePaper reads the folder as it is on disk, whatever branch is checked out.

That is all a team needs when the paper is written in LibrePaper. The steps below also put the paper in the repository.

### The paper in your own editor

To write the `.qmd` in RStudio, Positron or VS Code, connect the project to the same repository.

1. In the desktop app, open the [Git panel](../git.html#the-git-panel), click **Browse…** and choose your clone, then click **Connect**.
2. Click **Send project changes**. The `.qmd`, the bibliography and the images arrive in the repository.
3. Edit the `.qmd` in your editor and commit. Click **Get repository changes**, and your commits appear in the project as named versions under your name.
4. Click **Send project changes** again whenever you want your coauthors' web edits in the repository.

Changes move when you click, not keystroke by keystroke. Without the desktop app, `librepaper pull` and `librepaper push` do the same from a terminal.

Scripts never go to the project. Data files of a kind LibrePaper accepts, such as `.csv`, do go with everything else, and every collaborator can then read them. Keep private ones out with `.gitignore` or the `-librepaper` attribute. See [What syncs](../git.html#what-syncs).

While a file differs between the project and the repository, LibrePaper tells you when you render. Sync to make them agree.

## Figures

| The figure | Where it is | What to expect |
|---|---|---|
| A picture you upload, such as a diagram or a logo | The LibrePaper project | Use it in Markdown: `![](img/diagram.png)`. Everyone sees it. Code cannot read it. |
| A file already in your Run directory | Your computer | Markdown and code can both use it, while **Run code** is on. Coauthors need their own copy, through Dropbox or Git. |
| A plot your code draws in a cell | The preview, on your computer | It appears in your preview. There is no file to manage. |

## Share results

Share results works for Quarto documents rendered to HTML. Next to **Run code**, it is on by default. While it is on, each time you run code on your computer, LibrePaper keeps the output of your last run: tables, figures and printed values. It shows that output under each code cell in the browser draft, so collaborators who lack your data can still read the results.

- Each run replaces the shared results. Output from a cell whose code has changed since is shown faded, until the next run.
- Values written inline in the text, such as `r mean(x)`, are not shared yet.
- Shared results count against the storage of the document's owner. To delete them, go to **Settings → Storage**. To stop sharing, turn off **Share results**.

## Common hurdles

**A file is not found.** Code looks in your Run directory. Check that the file is there and that the path is relative to it.

**A file you uploaded to LibrePaper is not found by code.** Code cannot read the project's files. LibrePaper tells you when this is why a render failed. Keep data in your Run directory.

**The `.qmd` is in a subfolder.** Paths in code still start at the top of the Run directory, not at the subfolder. Quarto alone would start at the subfolder. Markdown paths are relative to the `.qmd`, as usual.

**The same file is in both places.** Markdown uses the LibrePaper copy. Code reads the one in your Run directory. LibrePaper tells you when this happens. Identical copies are fine.

**An image works in Markdown but not in `knitr::include_graphics()`.** Code only sees the Run directory. Keep that image there, or use Markdown `![](img/logo.png)`.

**Including a file your code writes.** Quarto reads `{{< include >}}` before code runs, so a file made in the same render cannot be included. It works from the next render.

**Coauthors keep data in different places.** Do not write anyone's folder into shared code. Use relative paths. Let someone whose data lives elsewhere override one path in their own `~/.Renviron`:

```r
harvest_root <- Sys.getenv("HARVEST_ROOT", "data/harvest_abstracts")
```

**Scripts (`.R`, `.py`, `.jl`) cannot be uploaded.** A script is a file your document reads, so it stays on your computer. Put scripts in your Run directory. Scripts uploaded before this rule are not shown in the project, but downloading the project (**File → Download**) still includes them.

**What uploading a folder or zip keeps.** Kept: `.qmd`, `.md`, `.bib`, `.csl`, `.yml`, `.css`, images and `.lua` filters. Left out: rendered output, `_freeze`, `_site`, `_book`, `*_files`, `*_cache`, `renv` and `venv` folders, data files such as `.csv`, and scripts. To share one data file anyway, list it in `.librepaper-share.json` at the top of the project:

```json
{"include": ["data/public.csv"]}
```

**Not visible to Quarto from the Run directory:** hidden files and folders, `_quarto.yml`, and folders Quarto makes (`*_files`, `*_cache`, `_freeze`).

**Windows network drives.** A Run directory on a network drive cannot be linked. Your code runs there, but Markdown does not see its files.

## Safety

Quarto runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not limit it. Code runs only while **Run code** is on, and LibrePaper never remembers this setting. Every editor can change the code, so turn on **Run code** only for documents whose editors you trust.
