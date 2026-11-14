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

If the desktop app does not find Calepin, install it or put it on PATH.

## Run

In Settings → Rendering, set Typst to render on this computer. A **Run code** button then appears at the top of the editor for Typst documents.

Code runs only when you press **Run code**. Each press runs the whole document once. **Stop** ends a run. Nothing runs when a document opens or while you type.

A plain Typst document with no code runs the same way. Calepin compiles it.

The entry file should be at the top of the project. Calepin keeps its `.calepin` folder beside the file it compiles.

## Run directory

The first time, LibrePaper asks for a Run directory: a folder on your computer with your data and scripts. It is never uploaded. Each person picks their own. The suggested folder is `~/LibrePaper/<project>`. The **Run code** button's menu shows the folder and lets you change it.

Calepin runs chunks in the project's working copy on your computer. The contents of your Run directory are linked into it, so relative paths such as `read.csv("data/survey.csv")` find the files there. Hidden files and folders in the Run directory are not linked.

Quarto differs here. Its code runs inside the Run directory itself. With Calepin, a file your code writes at the top level lands in the working copy, not in the Run directory. A file written inside a folder that comes from the Run directory lands in the Run directory.

The project's Typst source and resources are stored on the server, and anyone you share the project with can read them. Keep private data in your Run directory, not in the project.

## Preview and results

Between runs, the preview is always the browser's own Typst draft, PDF or HTML, as chosen in the View menu. No code runs.

After a run, the draft shows the results Calepin stored: output under each chunk, figures, and inline values. Prose edits show at once. A chunk whose code you changed keeps showing the code and output of the last run until you press **Run code** again. The **Run code** button shows how many chunks have changed since the last run.

If you add a chunk after a run, it has no stored output. The draft is drawn without outputs until the next run, and a note in Diagnostics says so. A notebook that has never been run still previews. Its code is shown without output.

## Publish results

**Publish results** is a tick box in the **Run code** button's menu. It is on by default, and each person sets it for each project.

While it is on, each run sends the stored results to the server. Collaborators see them in their own browser draft, including people without the desktop app or your data. Each run replaces the published results. Unlike Quarto, this works for PDF and HTML output alike, and inline values are included.

Published results count against the storage of the document's owner. To delete them, go to **Settings → Storage**. To stop, untick **Publish results**. Results are uploaded only while it is on.

## Common hurdles

**A file is not found.** Code looks in your Run directory, through the working copy. Check that the file is there and that the path is relative to it.

**Calepin is not found.** The desktop app could not run it. Install Calepin or put it on PATH. Settings → Rendering shows the commands the app runs, `calepin compile {input} --format pdf` and `calepin compile {input} --format html`, and lets you edit them.

**Outputs disappeared after adding a chunk.** A new chunk has no stored output until the next run. Press **Run code**.

**A font is missing.** Fonts uploaded to the project are not passed to Calepin. Install the font on your computer, or edit the command in Settings → Rendering.

## Safety

Calepin runs as you. It can read your files, use your packages and reach the internet. LibrePaper does not sandbox it. Code runs only when you press **Run code**. Every editor can change the code, so press **Run code** only on documents whose editors you trust.
