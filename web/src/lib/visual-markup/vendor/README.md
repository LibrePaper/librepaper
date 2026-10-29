Files from codemirror-visual-markup (MIT), commit 8a9f37ae1bb46bbcebdaf1602d396c5136e43005:
https://github.com/TeXlyre/codemirror-visual-markup/tree/8a9f37ae1bb46bbcebdaf1602d396c5136e43005/src

Only src/core, src/languages, src/view, src/index.ts and src/styles.css are
copied. The bundled toolbar and dual editor (src/ui) are left out, since
LibrePaper has its own chrome. See LICENSE.

Changes from upstream:

- view/math-field.ts no longer imports the optional MathLive peer, so the
  bundler never looks for it. LibrePaper draws math with its own KaTeX
  instead (see ../index.js).
- view/visual-editor.ts exports a `visualDefaults` facet whose values seed
  the initial visual state, so the mode can start on from configuration
  rather than with a dispatch after creation.
- view/decorations.ts subtracts ranges in one sweep rather than a quadratic
  loop, and `buildDecorations` takes previously made tokens as an optional
  third argument.
- view/visual-editor.ts keeps the parse across cursor moves, maps the
  decorations through an edit and rebuilds once typing pauses. The
  `refreshVisual` effect forces that rebuild.
- view/decorations.ts reveals an environment's \begin and \end only when the
  caret is on them, and languages/latex hides commands that draw nothing, such
  as \label and \bibliography.
