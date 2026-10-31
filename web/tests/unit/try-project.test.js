// The /try sandbox's file set: one copy of each main file under its demo
// name, one shared references.bib, and a welcome note in each format's syntax.
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildTryFiles } from "../../src/lib/try-files.js";

const byFormat = {
  typst: [
    { path: "main.typ", text: "#import \"x\"\n" },
    { path: "references.bib", text: "typst bib" },
  ],
  latex: [
    { path: "main.tex", text: "\\documentclass{article}\n" },
    { path: "references.bib", text: "latex bib" },
  ],
  markdown: [
    { path: "main.md", text: "---\nbibliography: references.bib\n---\n\n# Title\n" },
    { path: "references.bib", text: "markdown bib" },
  ],
  html: [
    { path: "main.html", text: "<!doctype html>\n" },
    { path: "chapters/01.html", text: "html chapter" },
  ],
};

const files = buildTryFiles(byFormat);
const pathsOf = (list) => list.map((file) => file.path);
const textOf = (path) => files.find((file) => file.path === path)?.text;

test("each main file takes its demo name", () => {
  assert.equal(textOf("main.typ")?.includes("#import"), true);
  assert.equal(textOf("latex.tex")?.includes("\\documentclass"), true);
  assert.equal(textOf("markdown.md")?.includes("# Title"), true);
  assert.equal(textOf("page.html")?.includes("<!doctype html>"), true);
  assert.equal(textOf("main.tex"), undefined);
  assert.equal(textOf("main.md"), undefined);
  assert.equal(textOf("main.html"), undefined);
});

test("references.bib is kept once, from the typst set", () => {
  const bibs = files.filter((file) => file.path === "references.bib");
  assert.equal(bibs.length, 1);
  assert.equal(bibs[0].text, "typst bib");
});

test("other files keep their path and are not duplicated", () => {
  assert.deepEqual(pathsOf(files).filter((path) => path.startsWith("chapters/")), ["chapters/01.html"]);
  assert.equal(new Set(pathsOf(files)).size, pathsOf(files).length);
});

test("a colliding file from a later format is dropped", () => {
  const colliding = buildTryFiles({
    ...byFormat,
    html: [...byFormat.html, { path: "chapters/01.html", text: "dropped" }],
  });
  assert.deepEqual(colliding.filter((file) => file.path === "chapters/01.html").map((file) => file.text), ["html chapter"]);
});

test("welcome notes use each format's comment syntax", () => {
  assert.match(textOf("main.typ"), /^\/\/ This is the LibrePaper demo sandbox\./);
  assert.match(textOf("latex.tex"), /^% This is the LibrePaper demo sandbox\./);
  assert.match(textOf("page.html"), /^<!-- This is the LibrePaper demo sandbox\. .* -->\n\n<!doctype html>/);
});

test("markdown note goes after the front matter", () => {
  const text = textOf("markdown.md");
  assert.match(text, /^---\nbibliography: references\.bib\n---\n<!-- This is the LibrePaper demo sandbox\./);
  assert.ok(text.indexOf("<!--") > text.indexOf("\n---\n"));
});

test("markdown without front matter gets the note at the top", () => {
  const plain = buildTryFiles({ ...byFormat, markdown: [{ path: "main.md", text: "# Plain\n" }, { path: "references.bib", text: "x" }] });
  assert.match(plain.find((file) => file.path === "markdown.md").text, /^<!-- This is the LibrePaper demo sandbox\./);
});

test("a format without its main file is an error", () => {
  assert.throws(() => buildTryFiles({ ...byFormat, latex: [] }), /latex template has no main\.tex/);
});
