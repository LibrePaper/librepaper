---
title: "Migration"
---

## From Overleaf

Download the project from Overleaf, then upload the ZIP as a new LibrePaper project.

1. In Overleaf, open the project and choose Menu, then Download, then Source. This saves a ZIP of every file.
2. In LibrePaper, choose New project, then the Upload Files tab.
3. Drop the ZIP anywhere on the page, or press Browse files and choose it.
4. LibrePaper unpacks the archive and keeps its folders. A wrapper folder around the whole archive is removed.
5. Check the file list and the Open first choice, which is usually `main.tex`. Then press Create project.

Things to check:

- The browser compiler has pdfLaTeX and XeLaTeX. LuaLaTeX projects need changes before they compile.
- Automatic engine choice follows a `% !TEX program = xelatex` line, then the packages the document loads. To force one engine everywhere, set it in Settings under Render.
- BibTeX and Biber run on their own. Upload the `.bib` files with the rest of the project.
- Unsupported file types are skipped. LibrePaper lists them after the upload.
- Overleaf comments, tracked changes and history do not come across. Only the files do.
