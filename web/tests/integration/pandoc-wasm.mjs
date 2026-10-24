// A real conversion with the pinned pandoc.wasm: a project with a nested main
// file, a relative image and a bibliography becomes Typst that cites it.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { createPandoc } from "../../src/lib/pandoc/worker.js";

const path = fileURLToPath(new URL("../../wasm/pandoc.wasm", import.meta.url));

test("Pandoc converts a nested Markdown project to Typst with a bibliography", { timeout: 300000 }, async (t) => {
  if (!existsSync(path)) return t.skip("web/wasm/pandoc.wasm missing");
  const pandoc = await createPandoc(readFileSync(path));
  const files = {
    "paper/main.md": "---\nbibliography: ../references.bib\n---\n\nSee @knuth84.\n\n![Pixel](img/pixel.png)\n",
    "paper/img/pixel.png": Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10),
    "references.bib": "@book{knuth84, author = {Knuth, Donald}, title = {The TeXbook}, year = {1984}}\n",
  };
  const options = { from: "markdown", to: "typst", standalone: true, "input-files": ["paper/main.md"], "resource-path": ["paper"] };
  const { output } = await pandoc.convert(options, files);
  const text = new TextDecoder().decode(output);
  assert.ok(text.includes("#bibliography("), text);
  // A second conversion reuses the module and sees a fresh tree.
  const again = await pandoc.convert({ ...options, "input-files": ["paper/main.md"] }, files);
  assert.ok(again.output.length > 0);
  await assert.rejects(pandoc.convert({ ...options, "input-files": ["missing.md"] }, files), /missing\.md/);
});
