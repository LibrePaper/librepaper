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

// The fast path skips the full index when an edit touches no line that could
// name a figure or include. Each test uses its own file names so the
// single-entry memo of one test never serves another.
const figures = (tree) => referencedFigures(tree).map(({ path, line }) => [path, line]);

for (const [label, main, build] of [
  [
    "Quarto",
    "fast-q.qmd",
    (prose, path = "a.png") => `# Title\n\n${prose}\n\n![alt](${path})\n`,
  ],
  [
    "LaTeX",
    "fast-l.tex",
    (prose, path = "a.png") => `\\section{T}\n\n${prose}\n\n\\includegraphics[width=3cm,\n  height=2cm]\n  {${path}}\n`,
  ],
]) {
  test(`${label}: prose edits keep the result, and the array is a fresh copy`, () => {
    const texts = { [main]: build("some words") };
    const first = referencedFigures({ main, texts });
    texts[main] = build("some more words, edited");
    const second = referencedFigures({ main, texts });
    assert.deepEqual(second, first);
    assert.notEqual(second, first);
    second[0].path = "mutated";
    second.push({ path: "x", name: "x", file: main, line: 1 });
    assert.deepEqual(referencedFigures({ main, texts }), first);
  });

  test(`${label}: adding and removing an image reference changes the result`, () => {
    const texts = { [main]: build("words") };
    assert.equal(referencedFigures({ main, texts }).length, 1);
    texts[main] = `${build("words")}${main.endsWith(".tex") ? "\\includegraphics{b.png}\n" : "![b](b.png)\n"}`;
    assert.equal(referencedFigures({ main, texts }).length, 2);
    texts[main] = build("words");
    assert.equal(referencedFigures({ main, texts }).length, 1);
  });

  test(`${label}: a line inserted above an image updates its line`, () => {
    const texts = { [main]: build("words") };
    const before = referencedFigures({ main, texts })[0].line;
    texts[main] = `\n${texts[main]}`;
    assert.equal(referencedFigures({ main, texts })[0].line, before + 1);
  });

  test(`${label}: editing the path on a continuation line updates the result`, () => {
    const texts = { [main]: build("words") };
    assert.deepEqual(paths({ main, texts }), ["a.png"]);
    texts[main] = build("words", "other.png");
    assert.deepEqual(paths({ main, texts }), ["other.png"]);
  });

  test(`${label}: a change to the known files invalidates`, () => {
    const texts = { [main]: build("words") };
    const tree = { main, texts, digests: {} };
    assert.equal(referencedFigures(tree).length, 1);
    tree.digests = { "a.png": "x" };
    assert.deepEqual(figures(tree).map(([path]) => path), ["a.png"]);
    const other = { main, texts: { ...texts, "extra.txt": "x" }, digests: tree.digests };
    assert.equal(referencedFigures(other).length, 1);
  });
}

test("Quarto: an image whose alt text wraps is no figure, and joining the lines makes it one", () => {
  const main = "fast-wrap.qmd";
  const texts = { [main]: "![one\ntwo](a.png)\n" };
  assert.deepEqual(figures({ main, texts }), []);
  texts[main] = "![one two](a.png)\n";
  assert.deepEqual(figures({ main, texts }), [["a.png", 1]]);
});

test("LaTeX: clearing an option line between the command and its path (a blank line) drops the figure", () => {
  const main = "fast-blank.tex";
  const texts = { [main]: "\\includegraphics[width=3cm,\nheight=2cm,\n]\n{a.png}\n" };
  assert.deepEqual(paths({ main, texts }), ["a.png"]);
  texts[main] = "\\includegraphics[width=3cm,\n\n]\n{a.png}\n";
  assert.deepEqual(paths({ main, texts }), []);
});

test("Quarto: a fence opened by a token line hides the images it holds", () => {
  const main = "fast-fence.qmd";
  const texts = { [main]: "```\n![a](a.png)\n```\n" };
  assert.deepEqual(paths({ main, texts }), []);
  texts[main] = "text\n![a](a.png)\n```\n";
  assert.deepEqual(paths({ main, texts }), ["a.png"]);
});

test("Quarto: a multi-line include and its included file invalidate the result", () => {
  const main = "fast-inc.qmd";
  const texts = { [main]: "{{< include\n\n  part-a.qmd\n>}}\n", "part-a.qmd": "![x](x.png)\n", "part-b.qmd": "![y](y.png)\n" };
  assert.deepEqual(paths({ main, texts }), ["x.png"]);
  texts[main] = "{{< include\n\n  part-b.qmd\n>}}\n";
  assert.deepEqual(paths({ main, texts }), ["y.png"]);
  texts["part-b.qmd"] = "![z](z.png)\n";
  assert.deepEqual(paths({ main, texts }), ["z.png"]);
  delete texts["part-b.qmd"];
  assert.deepEqual(paths({ main, texts }), []);
});

test("Quarto: a blank line after the include path is part of the include", () => {
  const main = "fast-inc-tail.qmd";
  const texts = { [main]: "{{< include\npart.qmd\n\n>}}\n", "part.qmd": "![x](x.png)\n" };
  assert.deepEqual(paths({ main, texts }), ["x.png"]);
  texts[main] = "{{< include\npart.qmd\njunk\n>}}\n";
  assert.deepEqual(paths({ main, texts }), []);
  texts[main] = "{{< include\nother.qmd\njunk\n>}}\n";
  texts["other.qmd"] = "![o](o.png)\n";
  assert.deepEqual(paths({ main, texts }), []);
  texts[main] = "{{< include\nother.qmd\n\n>}}\n";
  assert.deepEqual(paths({ main, texts }), ["o.png"]);
  texts[main] = "{{< include\npart.qmd\n\n>}}\n";
  assert.deepEqual(paths({ main, texts }), ["x.png"]);
});
