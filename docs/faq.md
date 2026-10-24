---
title: "FAQ"
---

## How do I move a project from Overleaf?

- In Overleaf, open the project and choose Menu > Download > Source. This saves a ZIP of every file.
- In LibrePaper, choose New project > Upload Files and drop the ZIP on the page. Folders are kept, and a wrapper folder around the whole archive is removed.
- The browser compiler has pdfLaTeX and XeLaTeX. LuaLaTeX projects need changes before they compile.
- The engine follows a `% !TEX program = xelatex` line, then the packages the document loads. Settings > Render forces one engine everywhere.
- BibTeX and Biber run on their own. Upload the `.bib` files with the rest of the project.
- Unsupported file types are skipped and listed after the upload.
- Only the files come across. Overleaf comments, tracked changes and history do not.

## How do I use my own fonts in LaTeX and Typst?

- Upload `.ttf` or `.otf` files into the project. A `fonts/` folder keeps them out of the way.
- Typst: name the family. Font files anywhere in the project are found.

  ```typst
  #set text(font: "Fira Sans")
  ```

- LaTeX: use `fontspec` and name the file. `fontspec` switches the project to XeLaTeX. Fonts are found by file name only, not by family name. Drop `Path=` when the font file sits next to the file that loads it.

  ```latex
  \setmainfont{FiraSans-Regular.otf}[Path=fonts/, BoldFont=FiraSans-Bold.otf]
  ```

- pdfLaTeX cannot use `.ttf` or `.otf` files.
- `.woff` and `.woff2` files are for HTML. Typst and LaTeX cannot read them.
- HTML previews use the browser's default fonts. Use the PDF view to see your fonts.
- Everyone who can open the project, read links included, downloads the font files. Check that the font licence allows that.

## Is there a dark mode?

- Yes. The app follows your computer's setting by default.
- Settings > Appearance > Theme offers Light, Dark and System. The choice is remembered per browser.
- The rendered document keeps its own colours. Only the interface changes.
