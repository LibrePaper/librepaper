---
title: "Calepin"
---

[Calepin](https://vincentarelbundock.github.io/calepin/) brings executable code chunks into native [Typst](https://typst.app) documents, so reports, papers and slides can compute tables, numbers and figures.

A notebook is an ordinary `.typ` file. It begins with two lines that load Calepin, and it holds fenced code blocks, such as a Python block:

```typ
#import "/.calepin/calepin.typ" as calepin
#show: calepin.document
```

## Desktop app

The browser cannot run code. The [LibrePaper desktop app](../desktop-app.html) runs Calepin on your computer, with the tools installed there: Calepin, Typst 0.15 or newer, and Python, R or Julia as the document needs.

If the desktop app does not find Calepin, install it or put it on PATH. **Settings → Tools → Programs** shows what it found.

## Run

In **Settings → Rendering**, set **Typst renders in** to **This computer**. A **Run code** button then appears at the top of the editor for Typst documents.

Code runs only when you press **Run code**. Each press runs the whole document once, and **Stop** ends a run. Nothing runs when a document opens or while you type. A Typst document with no code runs the same way.

Keep the notebook at the top of the project when you can. A notebook in a subfolder still runs and previews, but its code runs from that subfolder, so a file in your Run directory is one level up (`../data/survey.csv`). Calepin reads paths that start with `/` from the notebook's folder.

## Run directory

The first time, LibrePaper asks for a Run directory: a folder on your computer with your data and scripts. It is never uploaded. Each person picks their own. The suggested folder is `~/LibrePaper/<project>`. To change it, use the **Run directory** item in the **Run code** menu, which shows the current folder.

Calepin runs chunks in the project's working copy on your computer. The contents of the Run directory are linked into it, so relative paths such as `read.csv("data/survey.csv")` find the files. Hidden files and folders in the Run directory are not linked.

Quarto differs here: its code runs inside the Run directory itself. With Calepin, a file your code writes at the top level lands in the working copy, not in the Run directory. A file written inside a folder that comes from the Run directory lands in the Run directory.

The project's Typst source and resources are stored on the server, and anyone you share the project with can read them. Keep private data in your Run directory, not in the project.

## Preview and results

Between runs, the preview is the browser's own Typst draft, as PDF or HTML per the View menu. No code runs.

After a run, the draft shows what Calepin stored: output under each chunk, figures, and inline values. Prose edits show at once. A chunk whose code you changed keeps showing the code and output of the last run until you press **Run code** again. **Run code** shows how many chunks have changed since the last run.

A chunk added after a run has no stored output. The draft is drawn without outputs until the next run, and Diagnostics says so. A notebook that has never been run still previews, with its code and no output.

## Publish results

**Publish results** is a tick box in the **Run code** menu. It is on by default, and each person sets it for each project.

While it is on, each run sends the stored results to the server, replacing the previous ones. Collaborators see them in their own browser draft, including people without the desktop app or your data. When a collaborator's run is shown, Diagnostics names whose run it is. Unlike Quarto, this works for PDF and HTML output alike, and inline values are included.

Published results count against the document owner's storage. To delete them, go to **Settings → Account → Account storage**, then **Storage by document**. To stop, untick **Publish results**. Nothing is uploaded while it is off.

## Common hurdles

**A file is not found.** Code looks in your Run directory, through the working copy. Check that the file is there and that the path is relative to it.

**Calepin is not found.** The desktop app could not run it. Install Calepin or put it on PATH. **Settings → Rendering** shows the **PDF command** and **HTML command** for Typst, which run `calepin compile {input} --format pdf` and `calepin compile {input} --format html` by default. Changing a command asks you to confirm it in a dialog on this computer. **Reset to default** restores it.

**Outputs disappeared after adding a chunk.** A new chunk has no stored output until the next run. Press **Run code**.

**A font is missing.** Fonts uploaded to the project are not passed to Calepin. Install the font on your computer, or change the command in **Settings → Rendering**.

## Safety

Calepin runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not sandbox it. Code runs only when you press **Run code**. Every editor can change the code, so press **Run code** only on documents whose editors you trust.
