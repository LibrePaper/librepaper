// The clipboard HTML converter, run on the fixtures in tests/fixtures/paste and
// on small inline trees. The expected strings are written out by hand.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { htmlToSource, pasteText } from "../../src/lib/paste-html.js";
import { parseHtmlTree } from "./helpers/html-tree.mjs";

const fixture = (name) => readFileSync(new URL(`../fixtures/paste/${name}`, import.meta.url), "utf8");
const tree = (html) => parseHtmlTree(html);
const source = (html, format) => htmlToSource(tree(html), format);

// The paragraph-level line of a Markdown or LaTeX list, one item per line.
const lines = (...rows) => rows.join("\n");

/* ------------------------------------------------------ Google Docs, exact */

const GOOGLE_LATEX = lines(
  "\\section{Quarterly notes}",
  "",
  "\\textbf{Revenue} rose to \\$150,000 in Q3, about \\textasciitilde{}40\\% of the \\textit{forecast}. Details \\href{https://example.com/report}{here}.",
  "",
  "\\textbf{Net margin} was flat.",
  "",
  "\\begin{itemize}",
  "  \\item Owned by finance",
  "  \\begin{itemize}",
  "    \\item Q3 targets",
  "    \\item Q4 targets",
  "  \\end{itemize}",
  "  \\item Open \\textbf{risks}",
  "\\end{itemize}",
  "",
  "\\begin{tabular}{l l}",
  "\\textbf{Region} & Revenue \\\\",
  "EMEA & R\\&D 1.2M \\\\",
  "\\end{tabular}",
  "",
  "Use R\\&D, snake\\_case, \\{braces\\}, C:\\textbackslash{}path, x\\textasciicircum{}2, \\#1 and @ref.",
  "",
  "It’s done — ok now.",
);

const GOOGLE_MARKDOWN = lines(
  "# Quarterly notes",
  "",
  "**Revenue** rose to \\$150,000 in Q3, about ~40% of the *forecast*. Details [here](https://example.com/report).",
  "",
  "**Net margin** was flat.",
  "",
  "- Owned by finance",
  "  - Q3 targets",
  "  - Q4 targets",
  "- Open **risks**",
  "",
  "| **Region** | Revenue |",
  "| --- | --- |",
  "| EMEA | R&D 1.2M |",
  "",
  "Use R&D, snake\\_case, {braces}, C:\\\\path, x^2, #1 and @ref.",
  "",
  "It’s done — ok now.",
);

test("Google Docs fixture converts to exact LaTeX", () => {
  assert.equal(htmlToSource(parseHtmlTree(fixture("google-docs.html")), "latex"), GOOGLE_LATEX);
});

test("Google Docs fixture converts to exact Markdown", () => {
  assert.equal(htmlToSource(parseHtmlTree(fixture("google-docs.html")), "markdown"), GOOGLE_MARKDOWN);
});

test("Google Docs outer <b style=font-weight:normal> does not bold the page", () => {
  const latex = htmlToSource(parseHtmlTree(fixture("google-docs.html")), "latex");
  assert.ok(!latex.includes("\\textbf{Quarterly notes}"));
  assert.ok(!latex.includes("\\textbf{Use R"));
  assert.ok(latex.includes("\\textbf{Revenue}"));
});

/* ------------------------------------------------------- browser, exact */

const BROWSER_LATEX = lines(
  "\\subsection{Install the tool}",
  "",
  "Run \\texttt{npm install} then \\textbf{restart} your \\textit{shell}. See \\href{https://example.com/docs}{the docs}.",
  "",
  "\\begin{itemize}",
  "  \\item Linux",
  "  \\begin{itemize}",
  "    \\item Debian",
  "    \\item Fedora",
  "  \\end{itemize}",
  "  \\item macOS",
  "\\end{itemize}",
  "",
  "\\begin{enumerate}",
  "  \\item First step",
  "  \\item Second step",
  "\\end{enumerate}",
  "",
  "\\begin{quote}",
  "Quoted line.",
  "\\end{quote}",
  "",
  "\\begin{verbatim}",
  "const x = 1;",
  "console.log(x);",
  "\\end{verbatim}",
  "",
  "\\begin{tabular}{l l}",
  "Name & Size \\\\",
  "\\hline",
  "app & 12 MB \\\\",
  "\\end{tabular}",
  "",
  "Line one\\\\",
  "Line two with a span and an image.",
  "",
  "Prices: \\$150,000, \\textasciitilde{}40\\%, \\#1, R\\&D, snake\\_case, \\{braces\\}, C:\\textbackslash{}path, x\\textasciicircum{}2, @ref.",
);

const BROWSER_MARKDOWN = lines(
  "## Install the tool",
  "",
  "Run `npm install` then **restart** your *shell*. See [the docs](https://example.com/docs).",
  "",
  "- Linux",
  "  - Debian",
  "  - Fedora",
  "- macOS",
  "",
  "3. First step",
  "4. Second step",
  "",
  "> Quoted line.",
  "",
  "```",
  "const x = 1;",
  "console.log(x);",
  "```",
  "",
  "| Name | Size |",
  "| --- | --- |",
  "| app | 12 MB |",
  "",
  "Line one\\",
  "Line two with a span and an image.",
  "",
  "Prices: \\$150,000, ~40%, #1, R&D, snake\\_case, {braces}, C:\\\\path, x^2, @ref.",
);

test("browser fixture converts to exact LaTeX", () => {
  assert.equal(htmlToSource(parseHtmlTree(fixture("browser.html")), "latex"), BROWSER_LATEX);
});

test("browser fixture converts to exact Markdown", () => {
  assert.equal(htmlToSource(parseHtmlTree(fixture("browser.html")), "markdown"), BROWSER_MARKDOWN);
});

/* --------------------------------------------- other fixtures, substrings */

test("Word fixture: flat MsoList paragraphs become nested lists, markers dropped", () => {
  const tokens = parseHtmlTree(fixture("word.html"));
  const md = htmlToSource(tokens, "markdown");
  assert.ok(md.includes("Work is **urgent** and **critical**."), md);
  assert.ok(md.includes("- Buy supplies\n  - Paper and pens\n- Book the room"), md);
  assert.ok(md.includes("1. Write the draft\n   1. Add figures"), md);
  assert.ok(!md.includes("·"), "the Symbol bullet is dropped");
  assert.ok(md.includes("Costs are estimated"), md);
  const latex = htmlToSource(tokens, "latex");
  assert.ok(latex.includes("\\section{Project plan}"), latex);
  assert.ok(latex.includes("\\textbf{urgent}"), latex);
  assert.ok(latex.includes("\\begin{itemize}\n  \\item Buy supplies\n  \\begin{itemize}\n    \\item Paper and pens"), latex);
  assert.ok(latex.includes("\\begin{enumerate}\n  \\item Write the draft"), latex);
  assert.ok(latex.includes("snake\\_case"), latex);
});

test("LibreOffice fixture: real nesting, bold and italic, font tags dropped", () => {
  const tokens = parseHtmlTree(fixture("libreoffice.html"));
  const latex = htmlToSource(tokens, "latex");
  assert.ok(latex.includes("\\subsection{Status}"), latex);
  assert.ok(latex.includes("\\textbf{Bold} and \\textit{italic} with a font tag."), latex);
  assert.ok(latex.includes("\\begin{itemize}\n  \\item Alpha\n  \\begin{itemize}\n    \\item Beta\n  \\end{itemize}\n  \\item Gamma\n\\end{itemize}"), latex);
  assert.ok(latex.includes("\\begin{enumerate}\n  \\item One\n  \\item Two\n\\end{enumerate}"), latex);
  assert.ok(latex.includes("Ends with 50\\% \\& more. Costs \\$5, snake\\_case, \\textasciitilde{}2 and x\\textasciicircum{}2."), latex);
  const md = htmlToSource(tokens, "markdown");
  assert.ok(md.includes("- Alpha\n  - Beta\n- Gamma"), md);
  assert.ok(md.includes("1. One\n2. Two"), md);
});

test("Google Docs fixture in Typst", () => {
  const typ = htmlToSource(parseHtmlTree(fixture("google-docs.html")), "typst");
  assert.ok(typ.includes("= Quarterly notes"), typ);
  assert.ok(typ.includes("*Revenue* rose to \\$150,000 in Q3, about \\~40% of the _forecast_."), typ);
  assert.ok(typ.includes('#link("https://example.com/report")[here]'), typ);
  assert.ok(typ.includes("- Owned by finance\n  - Q3 targets\n  - Q4 targets\n- Open *risks*"), typ);
  assert.ok(typ.includes("#table(\n  columns: 2,\n  [*Region*], [Revenue],\n  [EMEA], [R&D 1.2M],\n)"), typ);
  assert.ok(typ.includes("Use R&D, snake\\_case"), typ);
});

test("Google Docs fixture in Org", () => {
  const org = htmlToSource(parseHtmlTree(fixture("google-docs.html")), "org");
  assert.ok(org.includes("* Quarterly notes"), org);
  assert.ok(org.includes("*Revenue* rose to $150,000 in Q3, about ~40% of the /forecast/. Details [[https://example.com/report][here]]."), org);
  assert.ok(org.includes("- Owned by finance\n  - Q3 targets\n  - Q4 targets\n- Open *risks*"), org);
  assert.ok(org.includes("| *Region* | Revenue |\n|---+---|\n| EMEA | R&D 1.2M |"), org);
});

test("browser fixture in Typst and Org", () => {
  const typ = htmlToSource(parseHtmlTree(fixture("browser.html")), "typst");
  assert.ok(typ.includes("== Install the tool"), typ);
  assert.ok(typ.includes("`npm install`"), typ);
  assert.ok(typ.includes("table.header([Name], [Size]),"), typ);
  assert.ok(typ.includes("#quote(block: true)[\nQuoted line.\n]"), typ);
  assert.ok(typ.includes("+ First step"), typ);
  const org = htmlToSource(parseHtmlTree(fixture("browser.html")), "org");
  assert.ok(org.includes("** Install the tool"), org);
  assert.ok(org.includes("~npm install~"), org);
  assert.ok(org.includes("| Name | Size |\n|---+---|\n| app | 12 MB |"), org);
  assert.ok(org.includes("#+begin_src\nconst x = 1;\nconsole.log(x);\n#+end_src"), org);
  assert.ok(org.includes("#+begin_quote\nQuoted line.\n#+end_quote"), org);
});

test("Quarto takes the Markdown output", () => {
  assert.equal(htmlToSource(parseHtmlTree(fixture("google-docs.html")), "quarto"), GOOGLE_MARKDOWN);
});

/* ------------------------------------------------------------ unit cases */

test("a plain wrapper with no formatting falls back to plain text", () => {
  const html = '<meta charset="utf-8"><span style="color:#000">just text</span>';
  assert.equal(htmlToSource(parseHtmlTree(html), "latex"), null);
  assert.equal(pasteText({ html, text: "just text", format: "latex" }), "just text");
});

test("html, empty and unknown formats never convert", () => {
  const html = "<p><b>bold</b></p>";
  assert.equal(htmlToSource(parseHtmlTree(html), "html"), null);
  assert.equal(htmlToSource(parseHtmlTree(html), ""), null);
  assert.equal(pasteText({ html, text: "bold", format: "html" }), "bold");
  assert.equal(pasteText({ html, text: "bold", format: "" }), "bold");
  assert.equal(pasteText({ html: "", text: "plain", format: "latex" }), "plain");
});

test("plain: true pastes the plain text even when the HTML has formatting", () => {
  assert.equal(pasteText({ html: "<b>x</b>", text: "x", format: "markdown", plain: true }), "x");
});

test("a parse that throws falls back to the plain text", () => {
  const parse = () => {
    throw new Error("broken");
  };
  assert.equal(pasteText({ html: "<b>x</b>", text: "x", format: "markdown", parse }), "x");
});

test("pasteText runs the converter with an injected parser", () => {
  const parse = () => ({ tag: "body", attrs: {}, children: [{ tag: "b", attrs: {}, children: ["x"] }, " y"] });
  assert.equal(pasteText({ html: "<b>x</b> y", text: "x y", format: "markdown", parse }), "**x** y");
});

test("an empty bold run produces no markup", () => {
  const html = "<p>a<b> </b>b</p><p><i>x</i></p>";
  assert.equal(source(html, "latex"), "a b\n\n\\textit{x}");
});

test("adjacent bold spans merge into one run", () => {
  const html = "<p><b>Mar</b><b>gin</b> is up</p><p><i>x</i></p>";
  assert.equal(source(html, "latex"), "\\textbf{Margin} is up\n\n\\textit{x}");
});

test("bold content moves its edge spaces outside the markup", () => {
  assert.equal(source("<p>see <b> this </b>now</p><p><i>x</i></p>", "markdown"), "see **this** now\n\n*x*");
});

test("nested lists in every format", () => {
  const html = "<ul><li>A<ul><li>B</li></ul></li></ul><p><i>x</i></p>";
  assert.equal(source(html, "latex"), "\\begin{itemize}\n  \\item A\n  \\begin{itemize}\n    \\item B\n  \\end{itemize}\n\\end{itemize}\n\n\\textit{x}");
  assert.equal(source(html, "typst"), "- A\n  - B\n\n_x_");
  assert.equal(source(html, "markdown"), "- A\n  - B\n\n*x*");
  assert.equal(source(html, "org"), "- A\n  - B\n\n/x/");
});

test("ordered lists number sequentially in Markdown and honour start", () => {
  const html = '<ol start="4"><li>a</li><li>b</li></ol><p><i>x</i></p>';
  assert.equal(source(html, "markdown"), "4. a\n5. b\n\n*x*");
});

test("a table in every format", () => {
  const html = "<table><tr><th>K</th><th>V</th></tr><tr><td>a|b</td><td>2</td></tr></table>";
  assert.equal(source(html, "latex"), "\\begin{tabular}{l l}\nK & V \\\\\n\\hline\na|b & 2 \\\\\n\\end{tabular}");
  assert.equal(source(html, "typst"), "#table(\n  columns: 2,\n  table.header([K], [V]),\n  [a|b], [2],\n)");
  assert.equal(source(html, "markdown"), "| K | V |\n| --- | --- |\n| a\\|b | 2 |");
  assert.equal(source(html, "org"), "| K | V |\n|---+---|\n| a|b | 2 |");
});

test("inline code takes a longer fence when it holds backticks", () => {
  assert.equal(source("<p><code>a`b</code></p><p><i>x</i></p>", "markdown"), "``a`b``\n\n*x*");
});

test("a pre block with backticks gets a longer fence", () => {
  assert.equal(source("<pre>x ``` y</pre><p><i>x</i></p>", "markdown"), "````\nx ``` y\n````\n\n*x*");
});

test("blockquote in Markdown prefixes every line", () => {
  assert.equal(source("<blockquote><p>one</p><p>two</p></blockquote><p><i>x</i></p>", "markdown"), "> one\n>\n> two\n\n*x*");
});

test("links: bare URL, text link and a fragment link", () => {
  assert.equal(source('<p><a href="https://a.test">https://a.test</a></p><p><i>x</i></p>', "latex"), "\\url{https://a.test}\n\n\\textit{x}");
  assert.equal(source('<p><a href="#top">top</a> <a href="javascript:x()">go</a></p><p><i>x</i></p>', "markdown"), "top go\n\n*x*");
});

test("LaTeX headings map levels one to six", () => {
  const html = "<h1>a</h1><h4>b</h4><h6>c</h6>";
  assert.equal(source(html, "latex"), "\\section{a}\n\n\\paragraph{b}\n\n\\subparagraph{c}");
});

test("Markdown escapes line-start markers and keeps Unicode", () => {
  assert.equal(source("<p>2024. was <b>big</b></p><p>#tag and - dash</p>", "markdown"), "2024\\. was **big**\n\n\\#tag and - dash");
});

test("Word list markers and Symbol bullets never reach the output", () => {
  const html = "<p class=MsoListParagraphCxSpFirst style='mso-list:l0 level1 lfo1'><span style='font-family:Symbol'>·<span>&nbsp;&nbsp; </span></span>Only item</p>";
  assert.equal(source(html, "markdown"), "- Only item");
});
