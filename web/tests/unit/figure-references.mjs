import assert from "node:assert/strict";
import test from "node:test";
import { figurePath, referencedFigures } from "../../src/lib/figure-references.js";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";
import { loadTypstSyntaxForTests } from "../helpers/typst-syntax.mjs";

await loadTypstSyntaxForTests();
await loadOrgSyntaxForTests();

const paths = (tree) => referencedFigures(tree).map(({ path }) => path);

test("Markdown and Quarto resolve a figure from the main file's directory", () => {
  const tree = { main: "doc/main.qmd", texts: { "doc/main.qmd": "text\n\n![a](fig/a.png)\n![b](/root.png)\n![c](https://e.org/x.png)\n" } };
  assert.deepEqual(referencedFigures(tree), [
    { path: "doc/fig/a.png", name: "fig/a.png", file: "doc/main.qmd", line: 3 },
    { path: "root.png", name: "/root.png", file: "doc/main.qmd", line: 4 },
  ]);
});

test("a Quarto include is textual: its figures resolve from the main file's directory", () => {
  const tree = {
    main: "main.qmd",
    texts: { "main.qmd": "{{< include parts/one.qmd >}}\n", "parts/one.qmd": "![x](img/x.png)\n" },
  };
  assert.deepEqual(referencedFigures(tree).map(({ path, file, line }) => [path, file, line]), [["img/x.png", "parts/one.qmd", 1]]);
});

test("Typst resolves from the referencing file, and a leading slash is the root", () => {
  const tree = {
    main: "main.typ",
    texts: {
      "main.typ": '#include "chapters/one.typ"\n#image("top.png")\n',
      "chapters/one.typ": '#image("fig.png")\n#image("/shared/s.png")\n#image("../up.png")\n',
    },
  };
  assert.deepEqual(paths(tree).sort(), ["chapters/fig.png", "shared/s.png", "top.png", "up.png"]);
});

test("LaTeX uses the index resolver: root first, then beside the file, with extension guessing", () => {
  const tree = {
    main: "main.tex",
    texts: { "main.tex": "\\includegraphics{pics/a}\n\\includegraphics{gone}\n" },
    digests: { "pics/a.pdf": "x" },
  };
  assert.deepEqual(paths(tree), ["pics/a.pdf", "gone"]);
});

test("Org and HTML resolve from the main file's directory", () => {
  assert.deepEqual(paths({ main: "d/n.org", texts: { "d/n.org": "[[file:p.png]]\n" } }), ["d/p.png"]);
  assert.deepEqual(paths({ main: "d/i.html", texts: { "d/i.html": '<img src="p.png">' } }), ["d/p.png"]);
});

test("figurePath ignores URLs and honours the path rule", () => {
  assert.equal(figurePath({ format: "markdown", file: "a/b.md", main: "m/main.md", name: "http://x/y.png" }), null);
  assert.equal(figurePath({ format: "markdown", file: "a/b.md", main: "m/main.md", name: "y.png" }), "m/y.png");
  assert.equal(figurePath({ format: "typst", file: "a/b.typ", main: "m/main.typ", name: "y.png" }), "a/y.png");
});

test("a tree without its main text refers to nothing", () => {
  assert.deepEqual(referencedFigures({ main: "x.md", texts: {} }), []);
});

test("Typst follows local imports but not package imports", () => {
  const tree = {
    main: "main.typ",
    texts: {
      "main.typ": '#import "lib/mod.typ": f\n#import "@preview/x:1.0.0": g\n',
      "lib/mod.typ": '#image("m.png")\n',
    },
  };
  assert.deepEqual(paths(tree), ["lib/m.png"]);
});

test("a repeated call on the same tree gives equal results, in a fresh array", () => {
  const tree = { main: "cache-same.md", texts: { "cache-same.md": "![a](a.png)\n" } };
  const first = referencedFigures(tree);
  const second = referencedFigures(tree);
  assert.deepEqual(second, first);
  assert.notEqual(second, first);
});

test("changing the main text is reflected on the next call", () => {
  const texts = { "cache-main.md": "![a](a.png)\n" };
  assert.deepEqual(paths({ main: "cache-main.md", texts }), ["a.png"]);
  texts["cache-main.md"] = "![a](a.png)\n![b](b.png)\n";
  assert.deepEqual(paths({ main: "cache-main.md", texts }), ["a.png", "b.png"]);
  texts["cache-main.md"] = "![b](b.png)\n";
  assert.deepEqual(paths({ main: "cache-main.md", texts }), ["b.png"]);
});

test("an included file whose text changes is re-indexed", () => {
  const texts = { "cache-doc.qmd": "{{< include cache-part.qmd >}}\n", "cache-part.qmd": "![x](x.png)\n" };
  assert.deepEqual(paths({ main: "cache-doc.qmd", texts }), ["x.png"]);
  texts["cache-part.qmd"] = "![y](y.png)\n![z](z.png)\n";
  assert.deepEqual(paths({ main: "cache-doc.qmd", texts }), ["y.png", "z.png"]);
});
