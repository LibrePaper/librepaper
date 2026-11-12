The visual markup layer draws LaTeX sources in place: headings, emphasis,
lists, math, figures and tables over the text, which stays the document.
Markdown, Quarto, Org and Typst are drawn by the paper view (lib/paper)
instead; their language definitions here still serve the formatting toolbar
and the task model.

core/, languages/, view/, markup.ts and markup.css began as a copy of
[codemirror-visual-markup](https://github.com/TeXlyre/codemirror-visual-markup)
by Fares Abawi, taken at commit 8a9f37ae1bb46bbcebdaf1602d396c5136e43005 and
changed a great deal since. That code is under the MIT License; its notice is
in LICENSE beside this file.
