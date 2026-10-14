---
title: "Migration"
---

## From Overleaf

Download the project from Overleaf, then upload the ZIP into a new LibrePaper project.

1. In Overleaf, open the project and choose Menu, then Download, then Source. This saves a ZIP of every file.
2. In LibrePaper, choose New project and pick the blank LaTeX template.
3. In the Files panel, press Upload files and choose the ZIP. You can also drop the ZIP on the panel.
4. LibrePaper unpacks the archive and keeps its folders. A wrapper folder around the whole archive is removed.
5. Both projects have a `main.tex`, so LibrePaper asks what to do. Choose Keep both to save yours as `main (2).tex`.
6. Open the file menu on your main file and choose Set as main file. Delete the starter `main.tex`.

Things to check:

- The browser compiler has pdfLaTeX and XeLaTeX. LuaLaTeX projects need changes before they compile.
- Automatic engine choice follows a `% !TEX program = xelatex` line, then the packages the document loads. To force one engine everywhere, set it in Settings under Render.
- BibTeX and Biber run on their own. Upload the `.bib` files with the rest of the project.
- Unsupported file types are skipped. LibrePaper lists them after the upload.
- Overleaf comments, tracked changes and history do not come across. Only the files do.
