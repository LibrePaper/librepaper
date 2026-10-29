Files from codemirror-visual-markup (MIT), commit 8a9f37ae1bb46bbcebdaf1602d396c5136e43005:
https://github.com/TeXlyre/codemirror-visual-markup/tree/8a9f37ae1bb46bbcebdaf1602d396c5136e43005/src

Only src/core, src/languages, src/view, src/index.ts and src/styles.css are
copied; the bundled toolbar and dual editor (src/ui) are left out, since
LibrePaper has its own chrome. One change: view/math-field.ts no longer
imports the optional MathLive peer, so the bundler never looks for it.
A second change: view/visual-editor.ts exports a `visualDefaults` facet whose values seed the initial visual state, so the mode can start on from configuration instead of a dispatch after creation. A third change: view/visual-editor.ts reuses the parse on cursor moves and defers the full rebuild after a document edit until typing pauses, with a `refreshVisual` effect to force it.
LibrePaper draws math with its own KaTeX instead (see ../index.js). See LICENSE.
