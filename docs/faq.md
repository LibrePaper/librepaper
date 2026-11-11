---
title: "FAQ"
---

## How do I move a project from Overleaf?

1. In Overleaf, open the project and choose Menu > Download > Source. This saves a ZIP of every file.
2. In LibrePaper, choose New project > Upload Files and drop the ZIP on the page. Folders are kept, and a wrapper folder around the whole archive is removed.

The browser compiler has pdfLaTeX and XeLaTeX, and LuaLaTeX projects need changes before they compile. The engine follows a `% !TEX program = xelatex` line, then the packages the document loads. Settings > Render forces one engine everywhere. BibTeX and Biber run on their own, so upload the `.bib` files with the rest of the project.

Unsupported file types are skipped and listed after the upload. Only the files come across. Overleaf comments, tracked changes and history do not.

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

pdfLaTeX cannot use `.ttf` or `.otf` files. `.woff` and `.woff2` files are for HTML, and Typst and LaTeX cannot read them. HTML previews use the browser's default fonts, so use the PDF view to see your fonts. Everyone who can open the project, read links included, downloads the font files, so check that the font licence allows that.

## How much does it cost to run LibrePaper?

Very little. The key architectural decision in this software is that document compilation happens in the client's browser using WebAssembly. That dramatically reduces the server load, such that I (Vincent) can keep librepaper.org running free for everyone. Currently, I am not taking donations. My approximate costs, in US dollars, are:

| Item | Cost |
|---|---|
| Virtual server: app, database, web server | $10 to $15 a month |
| Object storage for user files | $0.008 per GB a month |
| Object storage for the asset mirror: renderers, fonts, LaTeX | $0.008 per GB a month |
| Object storage for hourly backups, plus a replica | $0.008 per GB a month; about 7 times the size of the live data, for three months of history and a replica |
| Network: downloads and storage requests | free |
| macOS app signing and notarization | $99 a year |
| Windows app signing | about $120 a year |
| Domains | about $25 a year |
| Mail | free |
| Code hosting, releases, container images, desktop builds | free |
| Sign-in | free |
| TLS certificates | free |
| Backup monitoring | free |
| Admin access | free |
| **Total** | **about $25 to $40 a month** |

Each account can currently store 100 MB of files. If LibrePaper gets very popular and costs get out of hand, I might accept donations, on a non-profit basis. No feature will ever be gated. The software will always be open source and self-hostable.
