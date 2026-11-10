// Org documents reach a PDF through LaTeX where the deployment has a LaTeX
// compiler: Pandoc, running in the browser as WebAssembly, writes a LaTeX
// source from the Org text, and the LaTeX renderer compiles it. This is the
// route for Org when LaTeX is available; without it, Org uses Typst.

import { pandoc } from "./client.js";
import { orgForPandoc } from "./org.js";
import { dirOf, pandocPath } from "./typst.js";

/// Turn an Org document into a tree the LaTeX renderer compiles to PDF.
///
/// `tree` is `{ main, texts, assets, ... }` with `main` an Org path. Citations
/// are written for biblatex (`\autocite`, `\printbibliography`), so the document
/// needs biblatex, which the LaTeX renderer provides. `run` is Pandoc, injected
/// by tests. The returned tree's main is the `.tex` file Pandoc wrote beside
/// the Org file.
export async function orgLatexTree(tree, { run = pandoc } = {}) {
  const texPath = pandocPath(tree, "tex");
  const org = orgForPandoc(tree.texts?.[tree.main] ?? "", { to: "latex" });
  const { output } = await run(
    {
      from: "org",
      to: "latex",
      standalone: true,
      "input-files": [tree.main],
      "resource-path": [dirOf(tree.main)],
      "cite-method": "biblatex",
      "number-sections": org.numberSections,
    },
    { ...tree.texts, ...tree.assets, [tree.main]: org.text },
  );
  return { ...tree, main: texPath, texts: { ...tree.texts, [texPath]: new TextDecoder().decode(output) } };
}
