Files from codemirror-visual-markup (MIT), commit 8a9f37ae1bb46bbcebdaf1602d396c5136e43005:
https://github.com/TeXlyre/codemirror-visual-markup/tree/8a9f37ae1bb46bbcebdaf1602d396c5136e43005/src

Only src/core, src/languages, src/view, src/index.ts and src/styles.css are
copied; the bundled toolbar and dual editor (src/ui) are left out, since
LibrePaper has its own chrome. One change: view/math-field.ts no longer
imports the optional MathLive peer, so the bundler never looks for it.
LibrePaper draws math with its own KaTeX instead (see ../index.js). See LICENSE.
