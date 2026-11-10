// The Org pre-pass for Pandoc: math environments, internal links and section
// numbering, checked on the text Pandoc will read.
import assert from "node:assert/strict";
import { test } from "node:test";
import { orgForPandoc } from "../../src/lib/pandoc/org.js";
import { expandOrgIncludes } from "../../src/lib/org-include.js";

const EQUATION_NUMBERING = '#set math.equation(numbering: "(1)")';

test("bare math environments are wrapped in $$", () => {
  const source = "Intro.\n\n\\begin{equation}\n\\label{eq:one}\ny = \\beta x\n\\end{equation}\n";
  assert.equal(orgForPandoc(source).text, "Intro.\n\n$$\\begin{equation}\n\\label{eq:one}\ny = \\beta x\n\\end{equation}$$\n");
  assert.equal(orgForPandoc("\\begin{align*}\na &= b\n\\end{align*}\n").text, "$$\\begin{align*}\na &= b\n\\end{align*}$$\n");
  assert.equal(orgForPandoc("\\begin{displaymath}x\\end{displaymath}").text, "$$\\begin{displaymath}x\\end{displaymath}$$");
  // Other environments are left as written.
  assert.equal(orgForPandoc("\\begin{center}\nx\n\\end{center}\n").text, "\\begin{center}\nx\n\\end{center}\n");
});

test("math inside a source block is not touched", () => {
  const source = "#+BEGIN_SRC latex\n\\begin{equation}\nx\n\\end{equation}\n#+END_SRC\n";
  assert.equal(orgForPandoc(source).text, source);
});

test("a numbered math environment asks for equation numbering", () => {
  assert.equal(orgForPandoc("\\begin{equation}\nx\n\\end{equation}\n").headerIncludes, EQUATION_NUMBERING);
  assert.equal(orgForPandoc("\\begin{align}\nx\n\\end{align}\n").headerIncludes, EQUATION_NUMBERING);
  // Starred and unnumbered environments do not.
  assert.equal("headerIncludes" in orgForPandoc("\\begin{align*}\nx\n\\end{align*}\n"), false);
  assert.equal("headerIncludes" in orgForPandoc("\\begin{math}\nx\n\\end{math}\n"), false);
  assert.equal("headerIncludes" in orgForPandoc("Plain text.\n"), false);
});

test("section numbering is on unless num:nil or num:0", () => {
  assert.deepEqual(orgForPandoc("* Methods\n").variables, { sectionnumbering: "1.1" });
  assert.deepEqual(orgForPandoc("#+OPTIONS: toc:nil num:nil\n* Methods\n").variables, {});
  assert.deepEqual(orgForPandoc("#+OPTIONS: num:0\n* Methods\n").variables, {});
});

test("an unnumbered document with a numbered equation keeps the equation numbering", () => {
  const result = orgForPandoc("#+OPTIONS: num:nil\n\\begin{equation}\nx\n\\end{equation}\n");
  assert.deepEqual(result.variables, {});
  assert.equal(result.headerIncludes, EQUATION_NUMBERING);
});

test("a link to a CUSTOM_ID heading becomes a citation to it", () => {
  const source = "* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [[#methods]], [[*Methods]] and [[*Methods][the section]].\n";
  assert.equal(
    orgForPandoc(source).text,
    "* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [cite:@methods], [cite:@methods] and [[#methods][the section]].\n",
  );
});

test("headings without a CUSTOM_ID get Pandoc's identifiers, with -1 and -2 for repeats", () => {
  const source = "* Data and Methods!\n* Data and Methods!\n\n[[#data-and-methods-1]] and [[*Data and Methods!]] and [[#data-and-methods]].\n";
  assert.equal(orgForPandoc(source).text, "* Data and Methods!\n* Data and Methods!\n\n[cite:@data-and-methods-1] and [cite:@data-and-methods] and [cite:@data-and-methods].\n");
  // Leading non-letters are dropped, and a title with no letters is "section".
  assert.equal(orgForPandoc("* 2024 Results\n\n[[#results]]\n").text, "* 2024 Results\n\n[cite:@results]\n");
  assert.equal(orgForPandoc("* !!!\n\n[[#section]]\n").text, "* !!!\n\n[cite:@section]\n");
});

test("a #+NAME target is cited, and a described link keeps its text", () => {
  const source = "#+NAME: fig:one\n#+CAPTION: A table\n| a | b |\n\nSee [[fig:one]] and [[fig:one][the table]].\n";
  assert.equal(orgForPandoc(source).text, "#+NAME: fig:one\n#+CAPTION: A table\n| a | b |\n\nSee [cite:@fig:one] and [[#fig:one][the table]].\n");
});

test("a \\label inside a math environment is a citable target", () => {
  const source = "\\begin{equation}\n\\label{eq:one}\ny = x\n\\end{equation}\n\nSee [[#eq:one]].\n";
  assert.equal(orgForPandoc(source).text, "$$\\begin{equation}\n\\label{eq:one}\ny = x\n\\end{equation}$$\n\nSee [cite:@eq:one].\n");
});

test("unknown targets, URLs, angle links and anchors are left as written", () => {
  const source = "See [[#nowhere]], [[nowhere]], [[https://example.com][site]], <fig:one> and <<anchor>>.\n";
  assert.equal(orgForPandoc(source).text, source);
});

test("with num:nil a link to a heading gets its title as text", () => {
  const source = "#+OPTIONS: num:nil\n* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [[#methods]] and [[*Methods][x]] and [[#methods]].\n";
  const result = orgForPandoc(source);
  assert.equal(result.text, "#+OPTIONS: num:nil\n* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [[#methods][Methods]] and [[#methods][x]] and [[#methods][Methods]].\n");
  assert.deepEqual(result.variables, {});
});

test("with num:nil a link to a named element is still a citation", () => {
  const result = orgForPandoc("#+OPTIONS: num:nil\n#+NAME: fig:one\n| a |\n\n[[fig:one]]\n");
  assert.ok(result.text.endsWith("[cite:@fig:one]\n"), result.text);
});

// The LaTeX target: math is kept as written, internal links become \autoref
// snippets, and section numbering is reported rather than set as a variable.
const latex = (source) => orgForPandoc(source, { to: "latex" });

test("for LaTeX, math is left as written and gets no Typst header", () => {
  const source = "Intro.\n\n\\begin{align}\nx &= 1\n\\end{align}\n";
  const result = latex(source);
  assert.equal(result.text, source);
  assert.equal("headerIncludes" in result, false);
  assert.deepEqual(result.variables, {});
});

test("for LaTeX, an internal link becomes an \\autoref snippet", () => {
  assert.equal(
    latex("* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [[#methods]] and [[*Methods]].\n").text,
    "* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee @@latex:\\autoref{methods}@@ and @@latex:\\autoref{methods}@@.\n",
  );
  assert.equal(
    latex("#+NAME: fig:one\n#+CAPTION: A table\n| a | b |\n\n[[fig:one]]\n").text,
    "#+NAME: fig:one\n#+CAPTION: A table\n| a | b |\n\n@@latex:\\autoref{fig:one}@@\n",
  );
  assert.equal(
    latex("\\begin{equation}\n\\label{eq:one}\ny = x\n\\end{equation}\n\nSee [[#eq:one]].\n").text,
    "\\begin{equation}\n\\label{eq:one}\ny = x\n\\end{equation}\n\nSee @@latex:\\autoref{eq:one}@@.\n",
  );
});

test("for LaTeX, a described link keeps its text and an unknown target is left alone", () => {
  const heading = "* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\n";
  const source = `${heading}[[#methods][the section]] and [[#nowhere]].\n`;
  assert.equal(latex(source).text, source);
  assert.equal(latex(`${heading}[[*Methods][x]] and [[#nowhere]].\n`).text, `${heading}[[#methods][x]] and [[#nowhere]].\n`);
});

test("for LaTeX, num:nil gives a heading link its title and reports unnumbered sections", () => {
  const result = latex("#+OPTIONS: num:nil\n* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [[#methods]] and [[*Methods][x]].\n");
  assert.equal(result.text, "#+OPTIONS: num:nil\n* Methods\n:PROPERTIES:\n:CUSTOM_ID: methods\n:END:\n\nSee [[#methods][Methods]] and [[#methods][x]].\n");
  assert.equal(result.numberSections, false);
});

test("numberSections is true unless num:nil or num:0, for either target", () => {
  assert.equal(latex("* Methods\n").numberSections, true);
  assert.equal(latex("#+OPTIONS: num:0\n* Methods\n").numberSections, false);
  assert.equal(orgForPandoc("* Methods\n").numberSections, true);
  assert.equal(orgForPandoc("#+OPTIONS: num:nil\n* Methods\n").numberSections, false);
});

test("a heading link into an included file resolves once the includes are expanded", () => {
  const tree = {
    main: "paper.org",
    texts: {
      "paper.org": "* Intro\nSee [[*Methods]].\n#+INCLUDE: \"sections/methods.org\"\n",
      "sections/methods.org": "* Methods\nText.\n",
    },
    assets: {},
  };
  // Without the expansion the main file alone has no Methods heading.
  assert.match(orgForPandoc(tree.texts["paper.org"]).text, /\[\[\*Methods\]\]/);
  const expanded = expandOrgIncludes(tree).texts["paper.org"];
  const out = orgForPandoc(expanded).text;
  assert.equal(out.includes("[[*Methods]]"), false);
  assert.match(out, /\[cite:@methods\]/);
});
