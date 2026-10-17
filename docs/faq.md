---
title: "FAQ"
---

## How do I use my own fonts in LaTeX and Typst?

- Upload `.ttf` or `.otf` files into the project. A `fonts/` folder keeps them out of the way.
- Typst: name the family. Font files anywhere in the project are found.

  ```typst
  #set text(font: "Fira Sans")
  ```

- LaTeX: use `fontspec` and name the file. `fontspec` switches the project to XeLaTeX. Fonts are found by file name only, not by family name. Drop `Path=` when the file sits next to the main file.

  ```latex
  \setmainfont{FiraSans-Regular.otf}[Path=fonts/, BoldFont=FiraSans-Bold.otf]
  ```

- pdfLaTeX cannot use `.ttf` or `.otf` files.
- `.woff` and `.woff2` files are for HTML. Typst and LaTeX cannot read them.
- HTML previews use the browser's default fonts. Use the PDF view to see your fonts.
- Everyone who can open the project, read links included, downloads the font files. Check that the font licence allows that.
