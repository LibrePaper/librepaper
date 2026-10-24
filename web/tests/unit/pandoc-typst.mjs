import assert from "node:assert/strict";
import { pandocPath, typstTree } from "../../src/lib/pandoc/typst.js";

const calls = [];
const run = async (options, files) => {
  calls.push({ options, files });
  return { output: new TextEncoder().encode("#set page()\n"), warnings: [], stderr: "" };
};
const last = () => calls.at(-1);

// The Typst source sits beside the main file and is the new main.
{
  const tree = {
    main: "paper/main.md",
    texts: { "paper/main.md": "# Title\n", "paper/refs.bib": "@book{a}" },
    assets: { "paper/fig.png": Uint8Array.of(1) },
    urls: { "paper/fig.png": "blob:x" },
  };
  const out = await typstTree(tree, { from: "markdown", run });
  assert.equal(out.main, "paper/main.pandoc.typ");
  assert.equal(out.texts["paper/main.pandoc.typ"], "#set page()\n");
  assert.equal(out.texts["paper/main.md"], "# Title\n");
  assert.equal(out.assets, tree.assets);
  assert.deepEqual(out.urls, tree.urls);
  const { options, files } = last();
  assert.equal(options.from, "markdown");
  assert.equal(options.to, "typst");
  assert.equal(options.standalone, true);
  assert.deepEqual(options["input-files"], ["paper/main.md"]);
  assert.deepEqual(options["resource-path"], ["paper"]);
  assert.equal(options.citeproc, undefined);
  assert.equal(options.variables["header-includes"], "#set bibliography(title: none)");
  assert.equal("bibliographystyle" in options.variables, false);
  assert.deepEqual(Object.keys(files).sort(), ["paper/fig.png", "paper/main.md", "paper/refs.bib"]);
}

// A document at the root resolves from ".".
{
  const out = await typstTree({ main: "main.qmd", texts: { "main.qmd": "hi" }, assets: {} }, { from: "markdown", run });
  assert.equal(out.main, "main.pandoc.typ");
  assert.deepEqual(last().options["resource-path"], ["."]);
}

// An existing file is never overwritten.
{
  const tree = { main: "a/main.md", texts: { "a/main.md": "x", "a/main.pandoc.typ": "mine" }, assets: { "a/main.pandoc-2.typ": Uint8Array.of(1) } };
  assert.equal(pandocPath(tree, "typ"), "a/main.pandoc-3.typ");
  const out = await typstTree(tree, { from: "markdown", run });
  assert.equal(out.main, "a/main.pandoc-3.typ");
  assert.equal(out.texts["a/main.pandoc.typ"], "mine");
}

// The style comes from `csl` before `bibliography-style`.
{
  const style = async (front) => {
    await typstTree({ main: "main.md", texts: { "main.md": `---\n${front}\n---\n\nBody\n` }, assets: {} }, { from: "markdown", run });
    return last().options.variables.bibliographystyle;
  };
  assert.equal(await style("csl: apa\nbibliography-style: ieee"), "apa");
  assert.equal(await style("bibliography-style: ieee"), "ieee");
  assert.equal(await style("title: Plain"), undefined);
}

// HTML input has no front matter and reads from "html".
{
  const out = await typstTree({ main: "site/index.html", texts: { "site/index.html": "<p>x</p>" }, assets: {} }, { from: "html", run });
  assert.equal(out.main, "site/index.pandoc.typ");
  assert.equal(last().options.from, "html");
  assert.equal("bibliographystyle" in last().options.variables, false);
}
