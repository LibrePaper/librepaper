// A browser-level check for the visual markup spike: the layer that draws
// LaTeX and Typst sources in their rendered form while they stay plain text.
//
// The Editor is mounted with the layer on, and every check reads the DOM and
// the shared document only. Formulas are typeset by KaTeX, concealed commands
// drop out of the visible text, a caret inside a formula shows its source,
// edits reach the shared text, the visual switch turns the layer off and on
// in place, Typst is drawn too, Markdown is drawn, and a Quarto paper with
// front matter, cells, cross-references and callouts is drawn. Timings for long
// files and caret moves are printed for information and asserted nowhere.

import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join, sep } from "node:path";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { stopBrowserProcess, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-visual-markup-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const katexDist = join(root, "web/node_modules/katex/dist");
const port = 19000 + Math.floor(Math.random() * 1000);

// The same five lines as the LaTeX check, repeated to make a long file.
const texSample = [
  "\\section{Introduction}",
  "Some \\textbf{bold} and \\emph{italic} text with $E = mc^2$ inline.",
  "\\begin{itemize}",
  "\\item first",
  "\\end{itemize}",
].join("\n");
const typstSample = "= Heading\nSome *bold* and $x^2 + y^2$ here.";
const markdownSample = "# Readme\n\nSome *emphasis* and $x$ here.";
// A Quarto paper: front matter, numbered sections, inline and block markup, a
// code cell with a label, an image figure, a captioned table, display and inline
// math, a callout, a theorem, citations, cross-references, a footnote and a
// shortcode. The caret stays on the "Caret here." line.
const paperSample = [
  "---",
  "title: \"Quarto Paper\"",
  "author:",
  "  - Ada Lovelace",
  "  - Bob Smith",
  "date: \"2026-01-02\"",
  "number-sections: true",
  "---",
  "",
  "# Introduction {#sec-intro}",
  "",
  "Some **bold**, *italic*, ~~strike~~ and `inline code` text with a [link](https://example.org) and <https://example.com>.",
  "",
  "- [ ] open task",
  "- [x] done task",
  "",
  "1. first step",
  "2. second step",
  "",
  "> A quoted line.",
  "",
  "```python",
  "print(\"hi\")",
  "```",
  "",
  "```{r}",
  "#| label: fig-plot",
  "#| fig-cap: \"A plot\"",
  "plot(1:10)",
  "```",
  "",
  "![A chart](chart.png){#fig-chart}",
  "",
  "| Name | Score |",
  "|:-----|------:|",
  "| A    |     1 |",
  "| B    |     2 |",
  "",
  ": Results {#tbl-res}",
  "",
  "$$ E = mc^2 $$ {#eq-energy}",
  "",
  "Inline $x^2$ math and a citation [@knuth] and @knuth wrote.",
  "",
  "::: {.callout-note}",
  "A note with text.",
  ":::",
  "",
  "::: {#thm-main}",
  "Every bounded monotone sequence converges.",
  ":::",
  "",
  "Cross references: @fig-chart, @tbl-res, @eq-energy, @sec-intro and @fig-plot.",
  "",
  "A note[^1] in text.",
  "",
  "{{< pagebreak >}}",
  "",
  "[^1]: The footnote text.",
  "",
  "Caret here.",
].join("\n");
// A whole LaTeX paper: a preamble, math environments, labels and a bibliography.
const fullSample = [
  "\\documentclass[11pt]{article}",
  "\\usepackage[T1]{fontenc}",
  "\\usepackage{amsmath}",
  "\\usepackage{booktabs,graphicx}",
  "\\begin{document}",
  "\\section{Results}",
  "The estimate is in Equation \\ref{eq:se}.",
  "\\subsection{Main~estimate}",
  "\\begin{equation}",
  "\\label{eq:se}",
  "SE = \\frac{\\sigma}{\\sqrt{n}}",
  "\\end{equation}",
  "\\begin{align}",
  "a &= b \\\\",
  "c &= d",
  "\\end{align}",
  "\\begin{multline}",
  "x + y \\\\",
  "= z",
  "\\end{multline}",
  "\\begin{eqnarray}",
  "a & = & b",
  "\\end{eqnarray}",
  "\\label{sec:results}",
  "\\newpage",
  "\\bibliographystyle{plain}",
  "\\bibliography{references}",
  "\\end{document}",
].join("\n");
const longSample = Array.from({ length: 600 }, () => texSample).join("\n");
const hugeSample = Array.from({ length: 5 }, () => longSample).join("\n");
// A main file with a title block, an input, a cross reference to a section
// and to an equation, a reference to a label that does not exist, and a
// labelled equation. The included file is numbered before the main file's
// own section.
const refsSample = [
  "\\documentclass{article}",
  "\\title{A Study}",
  "\\author{Ada \\and Bob}",
  "\\date{2020}",
  "\\begin{document}",
  "\\maketitle",
  "\\input{sections/intro}",
  "\\section{Method}\\label{sec:m}",
  "See Equation~\\eqref{eq:a} and Section~\\ref{sec:m} and \\ref{sec:none}, \\cref{eq:a}, \\Cref{sec:m}, \\autoref{eq:a} and \\cref{sec:m}.",
  "\\begin{equation}",
  "a = b \\label{eq:a}",
  "\\end{equation}",
  "\\end{document}",
].join("\n");
const introSample = "\\section{Intro}\nText.";
// A figure with a caption and a label, between two paragraphs of text.
const figureSample = [
  "\\documentclass{article}",
  "\\begin{document}",
  "Intro text.",
  "\\begin{figure}[h]",
  "\\centering",
  "\\includegraphics[width=0.5\\textwidth]{plot.png}",
  "\\caption{A plot.}",
  "\\label{fig:plot}",
  "\\end{figure}",
  "After text.",
  "\\end{document}",
].join("\n");
// A kitchen-sink paper: most of the text commands, the common environments and
// the citation commands. The caret stays on the "Caret here." line in the body.
const sinkSample = [
  "\\documentclass{article}",
  "\\usepackage[numbers]{natbib}",
  "\\newcommand{\\R}{\\mathbb{R}}",
  "\\newcommand{\\pkg}[1]{\\texttt{#1}}",
  "\\newtheorem{lemma}{Lemma}",
  "\\newacronym{ml}{ML}{machine learning}",
  "\\begin{document}",
  "\\textbf{a} {\\bfseries b} {\\small c} \\textsuperscript{2} \\verb|x_1|",
  "Dashes -- and --- with ``q'' and \\'e, \\\"o, \\ldots, \\LaTeX, \\copyright.",
  "Links \\href{https://example.org}{site}, \\url{https://a.b/~c}, a note\\footnote{note}, \\enquote{hi}.",
  "Units \\SI{3}{\\meter\\per\\second}, \\num{12345}, \\gls{ml}, \\pkg{numpy}, and $x \\in \\R$.",
  "\\begin{lemma}[Key] text \\end{lemma}",
  "\\begin{proof} done \\end{proof}",
  "\\begin{abstract} sum \\end{abstract}",
  "\\begin{quote} said \\end{quote}",
  "\\begin{center} mid \\end{center}",
  "\\begin{tikzpicture} \\draw (0,0); \\end{tikzpicture}",
  "\\tableofcontents",
  "\\citep[p.~3]{knuth}, \\citet{knuth}, \\cite{knuth}.",
  "L'homme~dit \\og{}oui\\fg{}, \\texorpdfstring{shown $x$}{pdf alt}.",
  "Caret here.",
  "\\end{document}",
].join("\n");
// A title block in the body. The preamble has a title that the body overrides,
// the authors take an optional argument and a \thanks, an affiliation is given,
// and a title after \maketitle must not be used.
// Multi-line blocks whose spacing CodeMirror must measure: a click at the middle
// of a drawn line has to land in that line.
const offsetSample = [
  "\\documentclass{article}",
  "\\newtheorem{theorem}{Theorem}",
  "\\title{Offsets}",
  "\\author{A}",
  "\\begin{document}",
  "\\maketitle",
  "\\begin{theorem}",
  "First line.",
  "Second line.",
  "Third line.",
  "\\end{theorem}",
  "\\begin{quote}",
  "One.",
  "Two.",
  "\\end{quote}",
  "\\newpage",
  "\\begin{proof}",
  "Step one.",
  "Step two.",
  "\\end{proof}",
  "Caret here.",
  "\\end{document}",
].join("\n");

const titleSample = [
  "\\documentclass{article}",
  "\\usepackage{authblk}",
  "\\title{Old}",
  "\\begin{document}",
  "\\title{New}",
  "\\author[1]{Ada\\thanks{x}}",
  "\\author[2]{Bob}",
  "\\affil[1]{Uni A}",
  "\\maketitle",
  "\\title{After}",
  "Text.",
].join("\n");
// A Typst kitchen-sink document. The setup lines at the top (page, text, heading
// numbering, show, import and a definition) fold into one chip, and the body
// holds the markup, link, figure, layout and unknown-call cases. The caret stays
// on the "Caret here." line in the body.
const typstSinkSample = [
  '#set page(paper: "a5", margin: 2cm)',
  '#set text(font: "Libertinus Serif", size: 11pt, lang: "en")',
  '#set heading(numbering: "1.")',
  "#show heading: it => it.body",
  '#import "@preview/x:0.1.0": *',
  "#let f(x) = x",
  "= Sink",
  "== Level two",
  "*strong* and _emph_ words with `code` inline.",
  "",
  "```rust",
  "fn main() {}",
  "```",
  "",
  "- item",
  "",
  "+ item",
  "",
  "/ Term: def",
  "",
  'Colour: #text(fill: red)[warm], #rgb("#ff0000") swatch, #underline[u], #strike[s], #highlight[h], #smallcaps[sc], #super[2].',
  'Links: #link("https://example.org")[site], https://typst.app, a note#footnote[note] and #quote[said].',
  "Symbols: #sym.dash.em, #sym.arrow.r, #emoji.rocket, -- and --- and ... and ~ here.",
  '#figure(image("a.png"), caption: [A plot]) <fig-a>',
  "As @fig-a shows, #lorem(5)",
  "#pagebreak()",
  "#outline()",
  '#bibliography("refs.bib")',
  "#my-func(1)[x]",
  "Caret here.",
].join("\n");

// A LaTeX file with a display equation on its own lines, between two text lines.
// The caret starts on the text line above it, so one step down reaches the source.
const vimSample = [
  "\\documentclass{article}",
  "\\begin{document}",
  "Intro line above the equation.",
  "\\begin{equation}",
  "E = mc^2",
  "\\end{equation}",
  "Text line below the equation.",
  "\\end{document}",
].join("\n");

// A change from another peer, the way editor-browser.mjs makes one: a separate
// document imports this one, is edited, and its update is imported back. It is
// copied into the entry, which defines LoroDoc.
const fromAPeer = (target, edit) => {
  const peer = new LoroDoc();
  peer.import(target.export({ mode: "update" }));
  edit(peer);
  peer.commit();
  target.import(peer.export({ mode: "update", from: target.oplogVersion() }));
};

// Runs in the page. Mounts an Editor on vimSample with the given keys, the layer
// on, and the caret at the start of line 3, the text line above the equation. It
// refers only to names the generated entry defines (joinSession, createClassComponent,
// Editor, tick, EditorView, samples), so its source is copied into the entry as it stands.
async function vimMount(keys) {
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  const value = joinSession({ send: () => {}, mayEdit: true });
  const file = value.addText("vim-equation.tex", samples.vim);
  const host = document.createElement("section");
  document.body.append(host);
  const component = createClassComponent({
    component: Editor,
    target: host,
    props: { session: value, format: "latex", file, visual: true, keys },
  });
  await tick();
  // Vim loads asynchronously; its status panel, mode class or fat cursor marks the end of the wait.
  const vimReady = () => Boolean(host.querySelector(".cm-vimMode, .cm-vim-panel, .cm-fat-cursor"));
  for (let attempt = 0; keys === "vim" && attempt < 50 && !vimReady(); attempt++) await wait(100);
  const view = EditorView.findFromDOM(host.querySelector(".cm-editor"));
  view.dispatch({ selection: { anchor: view.state.doc.line(3).from } });
  view.focus();
  await wait(200);
  window.vimHarness = { component, host, value, view };
  return { vimReady: keys === "vim" ? vimReady() : null };
}

// Runs in the page. Reads the mounted vimMount editor: where the caret is, whether
// it sits inside the equation's source, the visible text and the drawn displays.
function vimRead() {
  const { view, host } = window.vimHarness;
  const source = view.state.doc.toString();
  const start = source.indexOf("\\begin{equation}");
  const endTag = "\\end{equation}";
  const end = source.indexOf(endTag) + endTag.length;
  const head = view.state.selection.main.head;
  return {
    head,
    line: view.state.doc.lineAt(head).number,
    inside: head >= start && head < end,
    text: host.querySelector(".cm-content").innerText,
    displays: host.querySelectorAll(".cm-lv-math-display").length,
    vimMode: Boolean(host.querySelector(".cm-vimMode, .cm-vim-panel, .cm-fat-cursor")),
  };
}

// Runs in the page. Takes the vimMount editor out again.
function vimDestroy() {
  const { component, host, value } = window.vimHarness;
  component.$destroy();
  host.remove();
  value.leave();
  window.vimHarness = null;
}

// Runs in the page. It refers only to names the generated entry defines
// (component, session, ids, samples, tick, EditorView, fromAPeer), so its source is
// copied into the entry as it stands.
async function visualCheck() {
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  const settle = async () => {
    await tick();
    await wait(100);
  };
  const view = () => EditorView.findFromDOM(document.querySelector(".cm-editor"));
  const markCount = () => document.querySelectorAll('.cm-editor [class*="cm-lv-"]').length;
  const katexCount = () => document.querySelectorAll(".cm-editor .katex").length;
  const mathKatexCount = () => document.querySelectorAll(".cm-editor .cm-lv-math .katex").length;
  const mathWidgets = () => document.querySelectorAll(".cm-editor .cm-lv-math").length;
  const visibleText = () => document.querySelector(".cm-content").innerText;
  // Rendered editor rows whose visible text is empty.
  // A row holding only a widget, such as an image, is not blank.
  const blankRows = () => Array.from(document.querySelectorAll(".cm-editor .cm-line"))
    .filter((node) => node.innerText.trim() === "" && !node.querySelector(".cm-lv-widget")).length;
  const texts = (selector) => Array.from(document.querySelectorAll(`.cm-editor ${selector}`), (node) => node.textContent.trim());
  const imageCount = () => document.querySelectorAll(".cm-editor .cm-lv-image").length;
  // Polls for up to five seconds, so a slow KaTeX load has time to land.
  const poll = async (count) => {
    for (let attempt = 0; attempt < 50; attempt++) {
      if (count() > 0) return true;
      await wait(100);
    }
    return false;
  };
  const median = (values) => {
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.floor(sorted.length / 2)];
  };
  const summary = (values) => ({
    median: Math.round(median(values) * 100) / 100,
    max: Math.round(Math.max(...values) * 100) / 100,
  });

  await settle();
  const first = {
    katexShown: await poll(katexCount),
    markCount: markCount(),
    mathKatex: mathKatexCount(),
    visibleText: visibleText(),
    docUnchanged: view().state.doc.toString() === samples.tex,
  };

  // Check 1b: a keystroke schedules a rebuild about 100 ms later, so the new
  // formula is typeset only after the wait. The caret stays at 0, outside
  // every formula, so no formula is revealed.
  view().dispatch({ selection: { anchor: 0 } });
  await settle();
  const lineTwoEnd = view().state.doc.line(2).to;
  const insertion = " $a+b$";
  const formulasBefore = mathWidgets();
  view().dispatch({ changes: { from: lineTwoEnd, insert: insertion }, selection: { anchor: 0 } });
  const deferred = { before: formulasBefore, afterDispatch: mathWidgets() };
  await wait(400);
  deferred.later = mathWidgets();
  deferred.typeset = Array.from(document.querySelectorAll(".cm-editor .cm-lv-math .katex"))
    .some((node) => node.textContent.replace(/\s/g, "").includes("a+b"));
  // Take the formula out again so that later checks see the sample as it was.
  view().dispatch({ changes: { from: lineTwoEnd, to: lineTwoEnd + insertion.length } });
  await settle();

  // Check 2: a caret inside the formula shows its source.
  const mathAt = view().state.doc.toString().indexOf("$E = mc^2$");
  view().dispatch({ selection: { anchor: mathAt + 3 } });
  await settle();
  const revealed = {
    katex: mathKatexCount(),
    mathWidgets: document.querySelectorAll(".cm-editor .cm-lv-math").length,
    text: visibleText(),
  };

  // Check 2b: a caret move right after a keystroke reveals at once. The caret
  // leaves the formula, a keystroke lands on line 1, and the caret moves back
  // into the formula with no wait, so a pending rebuild must not leave stale
  // atomic ranges behind.
  view().dispatch({ selection: { anchor: 0 } });
  await settle();
  view().dispatch({ changes: { from: view().state.doc.line(1).to, insert: "x" } });
  const quickAt = view().state.doc.toString().indexOf("$E = mc^2$");
  view().dispatch({ selection: { anchor: quickAt + 3 } });
  await tick();
  const quick = { mathWidgets: mathWidgets(), text: visibleText() };
  await settle();

  // Check 3: an insertion reaches the shared document.
  const lineTwo = view().state.doc.line(2).from;
  view().dispatch({ changes: { from: lineTwo, insert: "X" }, selection: { anchor: lineTwo + 1 } });
  await settle();
  const edited = {
    doc: view().state.doc.toString(),
    shared: session.doc.getMap("files").get(ids.tex).toString(),
  };

  // Check 4: the switch turns the layer off and on without rebuilding the view.
  const viewBefore = view();
  view().dispatch({ selection: { anchor: 0 } });
  await settle();
  component.$set({ visual: false });
  await settle();
  const off = { marks: markCount(), katex: katexCount(), text: visibleText() };
  component.$set({ visual: true });
  await settle();
  const back = { marks: markCount(), katex: await poll(katexCount) };
  const sameView = view() === viewBefore;

  // Check 4b: a peer's change arrives while the layer is on. A separate document
  // inserts " and $y^3$" at the end of line 2, with the caret on line 1 at offset 3,
  // away from the change. The new formula is drawn after the wait and the caret
  // keeps its offset. A second peer change edits the source of the existing formula
  // (mc^2 to mc^3); the drawn formula follows it and the caret stays put.
  view().dispatch({ selection: { anchor: 3 } });
  await settle();
  const collabKatexBefore = mathKatexCount();
  const collabAt = view().state.doc.line(2).to;
  fromAPeer(session.doc, (peer) => peer.getMap("files").get(ids.tex).insert(collabAt, " and $y^3$"));
  await wait(400);
  const collab = {
    line2: view().state.doc.line(2).text,
    text: view().state.doc.toString(),
    katexBefore: collabKatexBefore,
    katexAfter: mathKatexCount(),
    head: view().state.selection.main.head,
  };
  const collabMc = view().state.doc.toString().indexOf("mc^2");
  fromAPeer(session.doc, (peer) => {
    const file = peer.getMap("files").get(ids.tex);
    file.delete(collabMc, "mc^2".length);
    file.insert(collabMc, "mc^3");
  });
  await wait(400);
  collab.editedText = view().state.doc.toString();
  collab.editedDrawn = Array.from(document.querySelectorAll(".cm-editor .cm-lv-math .katex"), (node) => node.textContent)
    .some((text) => /mc\^?3/.test(text));
  collab.editedHead = view().state.selection.main.head;

  // Check 5: Typst is drawn.
  component.$set({ file: ids.typ, format: "typst" });
  await settle();
  const typst = {
    katex: await poll(mathKatexCount),
    text: visibleText(),
    marks: markCount(),
  };

  // Check 6: Markdown is drawn. The caret goes to the end of the file, on the
  // plain text after the emphasis, so the emphasis markers stay hidden.
  component.$set({ file: ids.md, format: "markdown" });
  await settle();
  // The caret sits on the heading, so the line with the emphasis is drawn.
  view().dispatch({ selection: { anchor: 0 } });
  await settle();
  const markdown = { marks: markCount(), text: visibleText() };

  // Check 6a: a Quarto paper. The front matter folds into a title block with the
  // caret on a plain line, and the markup, cells, figures, tables, math, callouts,
  // theorems, citations and cross-references are drawn. A click on the front
  // matter shows its YAML.
  component.$set({ visual: true, file: ids.paper, format: "markdown" });
  await settle();
  await wait(800);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Caret here.") + 3 } });
  await settle();
  const paperKatexShown = await poll(katexCount);
  const frontmatter = document.querySelector(".cm-editor [class*='frontmatter']");
  const quarto = {
    katexShown: paperKatexShown,
    katex: katexCount(),
    katexErrors: document.querySelectorAll(".cm-editor .katex-error").length,
    tables: document.querySelectorAll(".cm-editor table").length,
    tableBoxes: texts(".cm-lp-table-box"),
    verbs: document.querySelectorAll(".cm-editor .cm-lp-verb").length,
    links: document.querySelectorAll(".cm-editor .cm-lp-link").length,
    frontmatterShown: Boolean(frontmatter),
    text: visibleText(),
  };
  quarto.headnums = texts(".cm-lp-headnum");
  frontmatter?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await settle();
  quarto.frontmatterClick = visibleText();

  // Check 6b: a LaTeX paper. The preamble folds into one block while the caret
  // is in the body, labels and the bibliography are hidden, math environments
  // are typeset, and a caret in the preamble shows the source again.
  component.$set({ visual: true, file: ids.full, format: "latex" });
  await settle();
  const fullKatexShown = await poll(mathKatexCount);
  // A file opened in visual mode draws the whole page: the caret has not moved
  // yet, so it is hidden and the preamble around it stays folded.
  const parked = {
    editor: document.querySelector(".cm-editor")?.classList.contains("cm-lv-parked") ?? false,
    cursor: getComputedStyle(document.querySelector(".cm-editor .cm-cursorLayer")).visibility,
    preambles: document.querySelectorAll(".cm-editor .cm-lp-preamble").length,
    documentclass: visibleText().includes("\\documentclass"),
  };
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("\\section{Results}") + 2 } });
  await settle();
  parked.afterMove = document.querySelector(".cm-editor")?.classList.contains("cm-lv-parked") ?? false;
  const fullPreambles = document.querySelectorAll(".cm-editor .cm-lp-preamble");
  const fullBody = {
    parked,
    katexShown: fullKatexShown,
    preambleCount: fullPreambles.length,
    preambleText: fullPreambles.length ? fullPreambles[0].textContent : "",
    katex: mathKatexCount(),
    katexErrors: document.querySelectorAll(".cm-editor .katex-error").length,
    blankRows: blankRows(),
    hiddenTexts: texts(".cm-lp-hidden"),
    labelTexts: texts(".cm-lp-label"),
    text: visibleText(),
  };
  // A click on the folded preamble shows its source, with the caret still in the body.
  const preambleBlock = document.querySelector(".cm-editor .cm-lp-preamble");
  preambleBlock?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await settle();
  const fullPreambleClick = {
    found: Boolean(preambleBlock),
    preambleCount: document.querySelectorAll(".cm-editor .cm-lp-preamble").length,
    text: visibleText(),
  };
  view().dispatch({ selection: { anchor: 5 } });
  await settle();
  const fullPreamble = {
    preambleCount: document.querySelectorAll(".cm-editor .cm-lp-preamble").length,
    text: visibleText(),
  };
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("\\end{document}") + 2 } });
  await settle();
  const fullEnd = { text: visibleText() };
  // Heading numbers: Results is section 1, so a number reading 1 must sit before
  // the Results line, either inside it or on an earlier line.
  const resultsLine = Array.from(document.querySelectorAll(".cm-editor .cm-line")).find((node) => node.textContent.includes("Results"));
  const fullHeadings = {
    line: Boolean(resultsLine),
    headnums: texts(".cm-lp-headnum"),
    before: resultsLine
      ? Array.from(document.querySelectorAll(".cm-editor .cm-lp-headnum"))
        .filter((node) => node.textContent.trim() === "1" && (node === resultsLine || resultsLine.contains(node)
          || Boolean(node.compareDocumentPosition(resultsLine) & Node.DOCUMENT_POSITION_FOLLOWING))).length
      : 0,
  };

  // A click on the display equation shows its source, with the caret on \end{document}.
  const displayShown = await poll(() => document.querySelectorAll(".cm-editor .cm-lv-math-display").length);
  document.querySelector(".cm-editor .cm-lv-math-display")
    ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await settle();
  const fullDisplay = { shown: displayShown, text: visibleText() };

  // Check 6c: the labels of the project. refs.tex has a title block, an input,
  // a section and an equation reference, and a reference to a missing label.
  // The caret sits inside the word Method, away from every asserted command.
  // Labels settle about 300 ms after a change, so the wait is 800 ms.
  component.$set({ visual: true, file: ids.refs, format: "latex" });
  await settle();
  await wait(800);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Method") + 2 } });
  await settle();
  const textOf = (selector) => document.querySelector(`.cm-editor ${selector}`)?.textContent.trim() ?? null;
  const include = document.querySelector(".cm-editor .cm-lp-include");
  const refsCheck = {
    title: {
      main: textOf(".cm-lp-title-main"),
      author: textOf(".cm-lp-title-author"),
      date: textOf(".cm-lp-title-date"),
    },
    include: {
      text: include?.textContent ?? null,
      open: Boolean(include?.querySelector(".cm-lp-include-open")),
    },
    refTexts: Array.from(document.querySelectorAll(".cm-editor .cm-lp-ref"), (node) => node.textContent.trim()),
    katexTexts: Array.from(document.querySelectorAll(".cm-editor .katex"), (node) => node.textContent),
    text: visibleText(),
  };

  // Check 6c2: the parts of the title block in refs.tex. A mousedown on a part
  // puts the caret at the start of its argument in the source, and the preamble
  // shows only the lines around that command. A title edit draws at once, and
  // the edit is taken out again afterwards.
  const partClick = async (selector) => {
    document.querySelector(`.cm-editor ${selector}`)?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    await settle();
  };
  const refsSource = view().state.doc.toString();
  await partClick(".cm-lp-title-main");
  const titleClick = {
    head: view().state.selection.main.head,
    text: visibleText(),
    chips: texts(".cm-lp-preamble"),
    main: textOf(".cm-lp-title-main"),
  };
  const titleEnd = refsSource.indexOf("A Study") + "A Study".length;
  view().dispatch({ changes: { from: titleEnd, insert: " II" } });
  await wait(300);
  const liveTitle = textOf(".cm-lp-title-main");
  view().dispatch({ changes: { from: titleEnd, to: titleEnd + " II".length } });
  await settle();
  const restored = view().state.doc.toString() === refsSource;
  await partClick('[data-part="author"][data-index="1"]');
  const authorClick = { head: view().state.selection.main.head, text: visibleText() };
  await partClick('[data-part="date"]');
  const dateClick = { head: view().state.selection.main.head };
  view().dispatch({ selection: { anchor: 5 } });
  await settle();
  const preambleAt5 = { chips: texts(".cm-lp-preamble"), text: visibleText() };
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("\\maketitle") + 2 } });
  await settle();
  const maketitleText = visibleText();

  // Check 6c3: a title block drawn from title.tex, a body title before \maketitle.
  // Its part click lands in the body argument and the preamble stays folded.
  component.$set({ visual: true, file: ids.title, format: "latex" });
  await settle();
  const sampleShown = await poll(() => document.querySelectorAll(".cm-editor .cm-lp-title-main").length);
  const titleSourceText = view().state.doc.toString();
  view().dispatch({ selection: { anchor: titleSourceText.indexOf("Text.") + 1 } });
  await settle();
  const sample = {
    shown: sampleShown,
    main: textOf(".cm-lp-title-main"),
    author: textOf(".cm-lp-title-author"),
    affils: texts(".cm-lp-title-affil"),
  };
  await partClick(".cm-lp-title-main");
  sample.click = { head: view().state.selection.main.head, preambles: texts(".cm-lp-preamble").length };

  const titleBlock = { titleClick, liveTitle, restored, authorClick, dateClick, preambleAt5, maketitleText, sample };

  // Check 6d: a figure. With the caret in the body the figure folds into a drawn
  // image; with the caret inside its caption the source shows and the image does
  // not; a click on the image shows the source again.
  component.$set({ visual: true, file: ids.figure, format: "latex" });
  await settle();
  await wait(800);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Intro") + 2 } });
  await settle();
  const figureImageShown = await poll(imageCount);
  const figureIntro = { imageShown: figureImageShown, images: imageCount(), blankRows: blankRows(), text: visibleText() };
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("A plot") + 2 } });
  await settle();
  const figureInside = { images: imageCount(), text: visibleText() };
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Intro") + 2 } });
  await settle();
  const figureImage = document.querySelector(".cm-editor .cm-lv-image");
  const figure = { hadImage: Boolean(figureImage), intro: figureIntro, inside: figureInside };
  figureImage?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await wait(200);
  figure.clicked = { text: visibleText() };

  // Check 6e: the kitchen-sink paper. The caret sits on the last body line, so the
  // preamble folds and each construct in the body is drawn. A click on the TikZ
  // chip shows its source.
  component.$set({ visual: true, file: ids.sink, format: "latex" });
  await settle();
  await wait(800);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Caret here.") + 6 } });
  await settle();
  const sinkKatexShown = await poll(katexCount);
  const tikzChip = Array.from(document.querySelectorAll(".cm-editor .cm-lp-pill, .cm-editor .cm-lp-chip")).find((node) => /tikz/i.test(node.textContent));
  const sink = {
    katexShown: sinkKatexShown,
    text: visibleText(),
    verbs: texts(".cm-lp-verb"),
    footnotes: document.querySelectorAll(".cm-editor .cm-lp-footnote").length,
    chips: texts(".cm-lp-pill, .cm-lp-chip"),
    links: document.querySelectorAll(".cm-editor .cm-lp-link").length,
    cites: document.querySelectorAll(".cm-editor .cm-lp-cite").length,
    katexErrors: document.querySelectorAll(".cm-editor .katex-error").length,
    tikzChip: Boolean(tikzChip),
    // A glyph drawn in place of source, such as ’ for ' or a space for ~, takes its own width: a list marker's 1.2em would leave a gap after it.
    glyphWidths: Array.from(document.querySelectorAll(".cm-editor .cm-lv-glyph"))
      .filter((node) => ["’", "«", "»", " "].includes(node.textContent))
      .map((node) => node.getBoundingClientRect().width / parseFloat(getComputedStyle(node).fontSize)),
    markerTexts: texts(".cm-lv-marker"),
  };
  tikzChip?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await settle();
  sink.tikzClicked = visibleText();

  // Check 6e2: a click at the middle of each drawn line lands in that line, so
  // block spacing is padding CodeMirror measures, not margin it skips.
  component.$set({ visual: true, file: ids.offset, format: "latex" });
  await settle();
  await wait(800);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Caret here.") + 6 } });
  await settle();
  const offsetMisses = [];
  for (const line of document.querySelectorAll(".cm-editor .cm-content > .cm-line")) {
    const rect = line.getBoundingClientRect();
    if (!line.textContent.trim() || rect.height === 0) continue;
    const at = view().posAtCoords({ x: rect.left + 4, y: rect.top + rect.height / 2 });
    const drawn = view().state.doc.lineAt(view().posAtDOM(line)).number;
    const hit = at === null ? null : view().state.doc.lineAt(at).number;
    if (hit !== drawn) offsetMisses.push({ text: line.textContent.trim(), drawn, hit });
  }
  // A caret inside a block shows the whole block as source, and nothing of it drawn.
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Second line.") + 2 } });
  await settle();
  const offset = { misses: offsetMisses, inBlock: visibleText(), envheads: document.querySelectorAll(".cm-editor .cm-lp-envhead-theorem").length };

  // Check 6f: the Typst kitchen-sink document. The caret sits on "Caret here.", so
  // the setup lines fold into one chip and each construct in the body is drawn.
  // A click on the setup chip shows its source.
  component.$set({ visual: true, file: ids.sinkTyp, format: "typst" });
  await settle();
  await wait(800);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("Caret here.") + 6 } });
  await settle();
  const setupChip = Array.from(document.querySelectorAll(".cm-editor .cm-lp-hidden"))
    .find((node) => node.textContent.trim().startsWith("Setup:"));
  // The innermost element whose whole text is "warm" carries the colour style.
  const warmStyle = Array.from(document.querySelectorAll(".cm-editor [style]"))
    .filter((node) => node.textContent.trim() === "warm")
    .pop()?.getAttribute("style") ?? null;
  const typstSink = {
    text: visibleText(),
    hidden: texts(".cm-lp-hidden"),
    setupChip: Boolean(setupChip),
    warmStyle,
    footnotes: document.querySelectorAll(".cm-editor .cm-lp-footnote").length,
    links: document.querySelectorAll(".cm-editor .cm-lp-link").length,
    swatches: document.querySelectorAll(".cm-editor .cm-lp-swatch").length,
    katexErrors: document.querySelectorAll(".cm-editor .katex-error").length,
  };
  // The caret is still on "Caret here.", so the code fence is out of the way: its
  // fence markers must not show, its code must, and the heading numbers are read here.
  const typstFence = { text: visibleText(), headnums: texts(".cm-lp-headnum") };
  setupChip?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await settle();
  typstSink.clicked = visibleText();

  // Check 7: timing, reported and not asserted. Each typing run makes 50
  // single character insertions at the middle of the file, one dispatch at a
  // time. The caret run makes 50 selection-only dispatches, each moving the
  // caret down one line from the middle, with the layer on.
  const measure = async (visual, file, format) => {
    component.$set({ visual, file, format });
    await settle();
    await wait(300);
    const line = Math.floor(view().state.doc.lines / 2);
    const lines = view().state.doc.lines;
    const samplesMs = [];
    for (let index = 0; index < 50; index++) {
      const at = view().state.doc.line(line).from;
      const start = performance.now();
      view().dispatch({ changes: { from: at, insert: "x" } });
      samplesMs.push(performance.now() - start);
    }
    return { lines, ...summary(samplesMs) };
  };
  const timingOn = await measure(true, ids.big, "latex");
  const timingOff = await measure(false, ids.bigOff, "latex");

  component.$set({ visual: true, file: ids.big, format: "latex" });
  await settle();
  await wait(300);
  const caretLine = Math.floor(view().state.doc.lines / 2);
  const caretMs = [];
  for (let index = 0; index < 50; index++) {
    const at = view().state.doc.line(caretLine + index).from;
    const start = performance.now();
    view().dispatch({ selection: { anchor: at } });
    caretMs.push(performance.now() - start);
  }
  const caret = { lines: view().state.doc.lines, ...summary(caretMs) };

  // Five times the long sample: 15,000 lines.
  const hugeOn = await measure(true, ids.huge, "latex");
  const hugeOff = await measure(false, ids.hugeOff, "latex");

  return {
    first,
    deferred,
    revealed,
    quick,
    edited,
    collab,
    off,
    back,
    sameView,
    typst,
    markdown,
    quarto,
    full: { body: fullBody, preambleClick: fullPreambleClick, preamble: fullPreamble, end: fullEnd, display: fullDisplay, headings: fullHeadings },
    refs: refsCheck,
    titleBlock,
    figure,
    sink,
    offset,
    typstSink,
    typstFence,
    timing:{ lines: timingOn.lines, on: { median: timingOn.median, max: timingOn.max }, off: { median: timingOff.median, max: timingOff.max } },
    caret,
    hugeTiming: { lines: hugeOn.lines, on: { median: hugeOn.median, max: hugeOn.max }, off: { median: hugeOff.median, max: hugeOff.max } },
  };
}

const source = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { LoroDoc } from "loro-crdt";`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `const samples = ${JSON.stringify({ tex: texSample, typst: typstSample, markdown: markdownSample, paper: paperSample, full: fullSample, long: longSample, huge: hugeSample, refs: refsSample, intro: introSample, figure: figureSample, sink: sinkSample, offset: offsetSample, title: titleSample, typstSink: typstSinkSample, vim: vimSample })};`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `document.documentElement.dataset.palette = "librepaper";`,
  `document.documentElement.dataset.mode = "light";`,
  `const host = document.createElement("main");`,
  `host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(host);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const ids = {`,
  `  tex: session.addText("paper.tex", samples.tex),`,
  `  typ: session.addText("notes.typ", samples.typst),`,
  `  md: session.addText("readme.md", samples.markdown),`,
  `  paper: session.addText("paper.qmd", samples.paper),`,
  `  full: session.addText("full.tex", samples.full),`,
  `  big: session.addText("big.tex", samples.long),`,
  `  bigOff: session.addText("big-off.tex", samples.long),`,
  `  huge: session.addText("huge.tex", samples.huge),`,
  `  hugeOff: session.addText("huge-off.tex", samples.huge),`,
  `  refs: session.addText("refs.tex", samples.refs),`,
  `  figure: session.addText("figure.tex", samples.figure),`,
  `  sink: session.addText("sink.tex", samples.sink),`,
  `  sinkTyp: session.addText("sink.typ", samples.typstSink),`,
  `  offset: session.addText("offset.tex", samples.offset),`,
  `  title: session.addText("title.tex", samples.title),`,
  `  intro: session.addText("sections/intro.tex", samples.intro),`,
  `};`,
  `const component = createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "latex", file: ids.tex, visual: true },`,
  `});`,
  `const fromAPeer = ${fromAPeer.toString()};`,
  `window.vimMount = ${vimMount.toString()};`,
  `window.vimRead = ${vimRead.toString()};`,
  `window.vimDestroy = ${vimDestroy.toString()};`,
  `window.visualCheck = ${visualCheck.toString()};`,
  `window.visualCheckReady = true;`,
  "",
].join("\n");

writeFileSync(entry, source);
let server;
let browser;
let socket;
// Browser errors and uncaught exceptions, gathered over CDP while the check runs.
const errors = [];
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    define: { __KATEX__: JSON.stringify("/katex/") },
    resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "visual-markup-check.js", cssFileName: "visual-markup-check" },
    },
  });

  // KaTeX is served where the build's __KATEX__ points, so that
  // createKatexLoader requests `/katex/katex.min.js` and
  // `/katex/katex.min.css`, and the stylesheet's fonts/ paths resolve below it.
  server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.split("?")[0]);
    if (path.startsWith("/katex/")) {
      const file = join(katexDist, path.slice("/katex/".length));
      if (file.startsWith(katexDist + sep) && existsSync(file)) {
        response.setHeader("Content-Type", file.endsWith(".woff2") ? "font/woff2" : contentType(file));
        response.end(readFileSync(file));
        return;
      }
      response.statusCode = 404;
      response.end();
      return;
    }
    const file = join(output, path.slice(1));
    if (path !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><body><link rel="stylesheet" href="/visual-markup-check.css"><script type="module" src="/visual-markup-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local visual markup server did not start");

  browser = spawn(requireChromiumExecutable(), [
    "--headless=new",
    // CodeMirror renders only the lines near the window; a tall window puts
    // every line of the longer samples in the DOM, where the checks read them.
    "--window-size=1280,20000",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore", detached: true });
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await wait(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.method === "Runtime.exceptionThrown") {
      const details = message.params.exceptionDetails;
      errors.push(details.exception?.description ?? details.text);
    }
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") {
      errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "));
    }
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };

  await send("Runtime.enable");
  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.visualCheckReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "Editor did not mount in Chromium");

  const result = await evaluate("visualCheck()");

  // Check 2c, run after the page checks: with the caret on the text line above a
  // display equation, one step down reveals its source, with plain keys (ArrowDown)
  // and with Vim (j). Keys go over CDP so that Vim sees them as the browser's own.
  const keyPress = async (key, code, keyCode) => {
    await send("Input.dispatchKeyEvent", { type: "keyDown", key, code, windowsVirtualKeyCode: keyCode });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: keyCode });
    await wait(300);
  };
  const keyRun = async (keys, key, code, keyCode) => {
    const mounted = await evaluate(`vimMount(${JSON.stringify(keys)})`);
    const before = await evaluate("vimRead()");
    await keyPress(key, code, keyCode);
    const after = await evaluate("vimRead()");
    await evaluate("vimDestroy()");
    return { mounted, before, after };
  };
  const plainKeys = await keyRun("default", "ArrowDown", "ArrowDown", 40);
  const vimKeys = await keyRun("vim", "j", "KeyJ", 74);

  // Check 1: the layer is on for LaTeX.
  assert.equal(result.first.katexShown, true, "KaTeX did not typeset the inline formula within five seconds");
  assert.ok(result.first.markCount > 0, "no visual markup decorations were drawn for a LaTeX file with the layer on");
  assert.ok(result.first.mathKatex > 0, "the inline formula was not drawn as a KaTeX widget inside .cm-lv-math");
  assert.equal(result.first.visibleText.includes("\\textbf{"), false, "\\textbf{ is still visible; the command was not concealed");
  assert.equal(result.first.docUnchanged, true, "the visual layer changed the document text");
  console.log("visual-markup-browser: LaTeX formulas are typeset and commands concealed, with the document unchanged");

  // Check 1b: a keystroke schedules a rebuild that typesets the new formula.
  assert.equal(result.deferred.later, result.deferred.before + 1, "the formula typed at the end of line 2 was not drawn after the deferred rebuild");
  assert.equal(result.deferred.typeset, true, "the formula typed at the end of line 2 did not typeset as KaTeX inside .cm-lv-math with a+b in it");
  console.log(`visual-markup-browser: a keystroke's formula is drawn after the deferred rebuild (widgets: ${result.deferred.before} before, ${result.deferred.afterDispatch} right after the dispatch, ${result.deferred.later} later)`);

  // Check 2: a caret inside a formula reveals its source.
  assert.equal(result.revealed.katex, 0, "the formula widget stayed when the caret moved into its source");
  assert.equal(result.revealed.mathWidgets, 0, "a math widget remained with the caret inside its source");
  assert.ok(result.revealed.text.includes("E = mc^2"), "the formula source was not shown with the caret inside it");
  console.log("visual-markup-browser: a caret inside a formula shows its source");

  // Check 2b: a caret move straight after a keystroke reveals at once.
  assert.equal(result.quick.mathWidgets, 0, "a math widget covered the formula when the caret moved in straight after a keystroke");
  assert.ok(result.quick.text.includes("E = mc^2"), "the formula source was not visible when the caret moved in straight after a keystroke");
  console.log("visual-markup-browser: a caret move straight after a keystroke reveals the source at once");

  // Check 3: edits reach the shared document.
  assert.ok(result.edited.shared.split("\n")[1].startsWith("X"), "the insertion did not reach the shared text for the file");
  assert.equal(result.edited.shared, result.edited.doc, "the shared text and the editor document disagree after an insertion");
  console.log("visual-markup-browser: an edit with the layer on reaches the shared document");

  // Check 4: the visual switch reconfigures the open editor.
  assert.equal(result.off.marks, 0, "visual markup decorations remained after the layer was switched off");
  assert.equal(result.off.katex, 0, "KaTeX output remained after the layer was switched off");
  assert.ok(result.off.text.includes("\\textbf{"), "the command was still concealed after the layer was switched off");
  assert.ok(result.back.marks > 0, "the decorations did not come back when the layer was switched on again");
  assert.equal(result.back.katex, true, "KaTeX did not come back when the layer was switched on again");
  assert.equal(result.sameView, true, "switching the visual layer rebuilt the editor");
  console.log("visual-markup-browser: the visual switch turns the layer off and on in place");

  // Check 4b: a peer's change arrives while the layer is on, and is drawn.
  assert.ok(result.collab.line2.endsWith(" and $y^3$"), `the peer's change did not land at the end of line 2; line 2: ${JSON.stringify(result.collab.line2)}`);
  assert.ok(result.collab.text.includes(" and $y^3$"), "the peer's \" and $y^3$\" is not in the editor text");
  assert.equal(result.collab.katexAfter, result.collab.katexBefore + 1, `the peer's formula was not drawn: ${result.collab.katexBefore} KaTeX formulas in .cm-lv-math before, ${result.collab.katexAfter} after`);
  assert.equal(result.collab.head, 3, "the caret offset changed when a peer inserted text after it");
  assert.ok(result.collab.editedText.includes("mc^3"), "the peer's mc^3 edit is not in the editor text");
  assert.equal(result.collab.editedDrawn, true, "no drawn formula shows the mc^3 edit a peer made inside its source");
  assert.equal(result.collab.editedHead, 3, "the caret offset changed when a peer edited a formula's source");
  console.log("visual-markup-browser: a peer's inserted formula is drawn, and a peer's edit inside a formula is drawn, with the caret unmoved");

  // Check 5: Typst is drawn.
  assert.equal(result.typst.katex, true, "a Typst formula was not drawn as KaTeX inside .cm-lv-math");
  console.log("visual-markup-browser: a Typst formula is drawn");

  // Check 6: Markdown is drawn. Its emphasis markers are hidden, and its math is typeset.
  assert.ok(result.markdown.marks > 0, "no visual markup decorations were drawn for readme.md");
  assert.ok(result.markdown.text.includes("emphasis"), "the emphasis text was not visible in readme.md");
  assert.equal(result.markdown.text.includes("*emphasis*"), false, "the emphasis markers are still visible in readme.md");
  console.log("visual-markup-browser: Markdown is drawn and its emphasis markers are hidden");

  // Check 6a: a Quarto paper is drawn. Its front matter is a title block that shows
  // its YAML after a click, and the source of each construct is hidden with the
  // caret on the last line.
  assert.equal(result.quarto.katexShown, true, "KaTeX did not typeset the formulas in paper.qmd within five seconds");
  for (const drawn of ["Quarto Paper", "Ada Lovelace", "Bob Smith", "Introduction", "Figure", "Table", "Equation", "Section 1", "Note", "Theorem 1"]) {
    assert.ok(result.quarto.text.includes(drawn), `paper.qmd does not draw '${drawn}'; visible text: ${result.quarto.text}`);
  }
  for (const source of ["**", "~~", "{#", ":::", "{{<", "](", "$$"]) {
    assert.equal(result.quarto.text.includes(source), false, `${source} is still visible in paper.qmd with the caret on its last line`);
  }
  // The footnote definition line is shown as source, so it is taken out before the
  // check; what must not show is the reference [^1] in the paragraph.
  const quartoBody = result.quarto.text.replace(/\[\^1\]:[^\n]*/g, "");
  assert.equal(quartoBody.includes("[^1]"), false, "the footnote reference [^1] is still visible in paper.qmd");
  assert.ok(result.quarto.katex >= 2, `expected at least 2 KaTeX formulas in paper.qmd, found ${result.quarto.katex}`);
  assert.equal(result.quarto.katexErrors, 0, "paper.qmd has KaTeX parse errors (.katex-error inside .cm-editor)");
  assert.equal(result.quarto.tables, 0, "the pipe table was laid out as a <table> inside the editor");
  assert.deepEqual(result.quarto.tableBoxes, ["Table: Results"], "the pipe table was not drawn as a box with its caption and no cells");
  assert.ok(result.quarto.verbs > 0, "the inline code was not drawn as .cm-lp-verb");
  assert.ok(result.quarto.links > 0, "the link was not drawn as .cm-lp-link");
  assert.equal(result.quarto.frontmatterShown, true, "no front matter title block was drawn for paper.qmd");
  assert.equal(result.quarto.text.includes("title:"), false, "the front matter YAML showed before a click on the title block");
  assert.ok(result.quarto.frontmatterClick.includes("title:"), "a click on the front matter title block did not reveal title:");
  console.log("visual-markup-browser: paper.qmd draws its front matter, markup, cells, figures, tables, math, callouts, theorems and references");
  // Heading numbers: number-sections is true, so the first section reads 1.
  assert.ok(result.quarto.headnums.includes("1"), `paper.qmd has no .cm-lp-headnum reading 1 with number-sections: true; headnums: ${JSON.stringify(result.quarto.headnums)}`);

  // Check 6b: a LaTeX preamble folds, labels show as small tags, the bibliography
  // and page break fold into chips, math environments are typeset, and the
  // preamble source shows with the caret in it.
  assert.equal(result.full.body.katexShown, true, "KaTeX did not typeset the math environments in full.tex within five seconds");
  assert.equal(result.full.body.preambleCount, 1, "the LaTeX preamble did not fold into exactly one .cm-lp-preamble block with the caret in the body");
  assert.match(result.full.body.preambleText, /Preamble: article, 4 packages/, "the preamble block did not read as 'Preamble: article, 4 packages'");
  // The label keys show as tags now, so only the command names are hidden here.
  // The bibliography chip names its style and database, so "references" is not
  // checked as absent.
  const parked = result.full.body.parked;
  assert.equal(parked.editor, true, "full.tex opened in visual mode without the caret parked");
  assert.equal(parked.cursor, "hidden", "the parked caret is still drawn in full.tex");
  assert.equal(parked.preambles, 1, "the preamble of full.tex is not folded before the caret moves");
  assert.equal(parked.documentclass, false, "\\documentclass shows as source in full.tex before the caret moves");
  assert.equal(parked.afterMove, false, "a caret move did not end the parked caret in full.tex");
  const hiddenInBody = [
    "\\documentclass", "\\usepackage", "\\label", "eq:se",
    "\\bibliographystyle", "\\bibliography", "\\newpage",
    "\\begin{document}", "\\end{document}", "\\begin{equation}", "\\begin{align}", "~",
  ];
  for (const source of hiddenInBody) {
    assert.equal(result.full.body.text.includes(source), false, `${source} is still visible in full.tex with the caret in the body`);
  }
  assert.equal(result.full.body.text.includes("sec:results"), false, "the \\label{sec:results} key is still visible in full.tex with the caret in the body");
  assert.ok(result.full.body.text.includes("Main estimate"), "the tie in \\subsection{Main~estimate} was not drawn as a space in full.tex");
  assert.deepEqual(result.full.body.labelTexts, [], `a \\label still shows as a .cm-lp-label tag: ${JSON.stringify(result.full.body.labelTexts)}`);
  assert.equal(result.full.body.hiddenTexts.some((text) => text.includes("\\label")), false, `a hidden-line chip names \\label: ${JSON.stringify(result.full.body.hiddenTexts)}`);
  for (const chip of ["Bibliography: references (plain)", "Page break", "End of document"]) {
    // Adjacent hidden lines share one chip that names each part.
    assert.ok(result.full.body.hiddenTexts.some((text) => text.split(" · ").includes(chip)), `no .cm-lp-hidden chip names '${chip}'; chips: ${JSON.stringify(result.full.body.hiddenTexts)}`);
  }
  const fullBlank = fullSample.split("\n").filter((line) => !line.trim()).length;
  assert.ok(result.full.body.blankRows <= fullBlank, `full.tex has ${result.full.body.blankRows} blank rendered rows; the source has ${fullBlank} empty lines`);
  assert.ok(result.full.body.katex >= 4, `expected at least 4 KaTeX formulas inside .cm-lv-math in full.tex, found ${result.full.body.katex}`);
  assert.equal(result.full.body.katexErrors, 0, "full.tex has KaTeX parse errors (.katex-error inside .cm-editor)");
  assert.equal(result.full.preamble.preambleCount, 0, "the preamble block stayed when the caret moved into the preamble");
  assert.ok(result.full.preamble.text.includes("\\documentclass"), "the preamble source did not show with the caret in the preamble");
  assert.ok(result.full.end.text.includes("\\end{document}"), "\\end{document} did not show with the caret on that line");
  assert.equal(result.full.preambleClick.found, true, "no .cm-lp-preamble block to click with the caret in the body");
  assert.ok(result.full.preambleClick.text.includes("\\documentclass"), "a click on the preamble block did not show its source");
  assert.equal(result.full.preambleClick.preambleCount, 0, "the preamble block stayed after a click on it");
  assert.equal(result.full.display.shown, true, "no .cm-lv-math-display was drawn in full.tex");
  assert.ok(result.full.display.text.includes("\\frac{\\sigma}"), "a click on the display equation did not show its source");
  console.log(`visual-markup-browser: full.tex folds its preamble, hides labels and the bibliography, and typesets ${result.full.body.katex} math environments`);
  // Heading numbers: \section{Results} is section 1, so a number reading 1 sits before its line.
  assert.equal(result.full.headings.line, true, "no drawn line holds the \\section{Results} heading in full.tex");
  assert.ok(result.full.headings.headnums.includes("1"), `no .cm-lp-headnum reads 1 in full.tex; headnums: ${JSON.stringify(result.full.headings.headnums)}`);
  assert.ok(result.full.headings.before > 0, "no .cm-lp-headnum reading 1 sits before the Results heading in full.tex");
  console.log("visual-markup-browser: full.tex numbers the Results section 1");

  // Check 6c: title, include, references and the labelled equation in refs.tex.
  assert.equal(result.refs.title.main, "A Study", "the \\title text was not drawn in .cm-lp-title-main");
  assert.ok(result.refs.title.author.includes("Ada") && result.refs.title.author.includes("Bob"), "the \\author names were not drawn in .cm-lp-title-author");
  assert.equal(result.refs.title.date, "2020", "the \\date text was not drawn in .cm-lp-title-date");
  assert.ok(result.refs.include.text.includes("sections/intro"), "the \\input name was not drawn in .cm-lp-include");
  assert.equal(result.refs.include.open, true, "the \\input widget has no .cm-lp-include-open button");
  assert.ok(result.refs.refTexts.includes("(1)"), `the \\eqref did not show (1); .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  assert.ok(result.refs.refTexts.includes("2"), `the \\ref to Method did not show 2; .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  assert.ok(result.refs.refTexts.includes("??"), `the \\ref to a missing label did not show ??; .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  for (const word of ["eq. (1)", "Section 2", "Equation 1", "section 2"]) {
    assert.ok(result.refs.refTexts.includes(word), `the reference word '${word}' was not drawn; .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  }
  for (const source of ["~", "\\ref", "\\eqref", "\\maketitle", "\\input"]) {
    assert.equal(result.refs.text.includes(source), false, `${source} is still visible in refs.tex with the caret in the body`);
  }
  assert.ok(result.refs.katexTexts.some((text) => text.includes("(1)")), "the labelled equation did not show its number (1) through KaTeX");
  console.log(`visual-markup-browser: refs.tex draws its title, include, references ${JSON.stringify(result.refs.refTexts)} and equation number`);

  // Check 6c2: the parts of the title block open their source lines in refs.tex.
  // The expected preamble summary is computed from the sample's own preamble.
  const refsPreamble = refsSample.slice(0, refsSample.indexOf("\\begin{document}"));
  const refsClass = refsPreamble.match(/\\documentclass(?:\[[^\]]*\])?\{([^}]*)\}/)[1];
  const refsPackages = [...refsPreamble.matchAll(/\\usepackage(?:\[[^\]]*\])?\{([^}]*)\}/g)]
    .reduce((count, match) => count + match[1].split(",").length, 0);
  const refsSummary = `Preamble: ${refsClass}, ${refsPackages} ${refsPackages === 1 ? "package" : "packages"}`;
  const tb = result.titleBlock;
  assert.equal(tb.titleClick.head, refsSample.indexOf("\\title{") + 7, "a click on the title did not put the caret at the start of the \\title argument");
  assert.ok(tb.titleClick.text.includes("\\title{A Study}"), "a click on the title did not show \\title{A Study}");
  for (const source of ["\\documentclass", "\\author", "\\maketitle"]) {
    assert.equal(tb.titleClick.text.includes(source), false, `${source} is still visible after a click on the title`);
  }
  assert.deepEqual(tb.titleClick.chips, [refsSummary, "More preamble"], `the preamble chips after a click on the title were ${JSON.stringify(tb.titleClick.chips)}`);
  assert.equal(tb.titleClick.main, "A Study", "the title block changed after a click on the title");
  assert.equal(tb.liveTitle, "A Study II", "an edit to the title did not draw in the title block within 300 ms");
  assert.equal(tb.restored, true, "the title edit was not taken out of refs.tex");
  assert.equal(tb.authorClick.head, refsSample.indexOf("Bob"), "a click on the second author did not put the caret at Bob");
  assert.ok(tb.authorClick.text.includes("\\author{Ada \\and Bob}"), "a click on the second author did not show \\author{Ada \\and Bob}");
  assert.equal(tb.authorClick.text.includes("\\title{"), false, "\\title{ is still visible after a click on the author");
  assert.equal(tb.dateClick.head, refsSample.indexOf("\\date{") + 6, "a click on the date did not put the caret at the start of the \\date argument");
  assert.deepEqual(tb.preambleAt5.chips, [], "the preamble fold stayed with the caret at offset 5");
  assert.ok(tb.preambleAt5.text.includes("\\date{2020}"), "\\date{2020} was not visible with the caret in the preamble");
  assert.ok(tb.maketitleText.includes("\\maketitle"), "\\maketitle was not revealed with the caret on it");
  assert.equal(tb.sample.shown, true, "no .cm-lp-title-main was drawn for title.tex");
  assert.equal(tb.sample.main, "New", `the title before \\maketitle was not drawn as New; got ${JSON.stringify(tb.sample.main)}`);
  assert.ok(tb.sample.author.includes("Ada") && tb.sample.author.includes("Bob"), "the authors of title.tex were not drawn in .cm-lp-title-author");
  assert.equal(tb.sample.author.includes("x"), false, "the \\thanks text showed in the author line of title.tex");
  assert.ok(tb.sample.affils.some((text) => text.includes("Uni A")), `the \\affil was not drawn in .cm-lp-title-affil; got ${JSON.stringify(tb.sample.affils)}`);
  assert.equal(tb.sample.click.head, titleSample.indexOf("\\title{New}") + 7, "a click on the body title did not put the caret at the start of its argument");
  assert.equal(tb.sample.click.preambles, 1, "the preamble did not stay folded after a click on the body title");
  console.log(`visual-markup-browser: the title block's parts open their source lines, the preamble splits around them, and the title draws its edits (${tb.titleClick.chips.join(" | ")})`);

  // Check 6d: a figure folds into an image with the caret outside it, shows its
  // source with the caret inside it, and shows its source after a click on the image.
  const figureBlank = figureSample.split("\n").filter((line) => !line.trim()).length;
  for (const source of ["\\begin{figure}", "\\centering", "\\includegraphics", "\\end{figure}"]) {
    assert.equal(result.figure.intro.text.includes(source), false, `${source} is still visible in figure.tex with the caret on Intro`);
  }
  assert.equal(result.figure.intro.imageShown, true, "no .cm-lv-image was drawn for the figure with the caret on Intro");
  assert.ok(result.figure.intro.blankRows <= figureBlank, `figure.tex has ${result.figure.intro.blankRows} blank rendered rows; the source has ${figureBlank} empty lines`);
  for (const source of ["\\begin{figure}", "\\includegraphics", "\\end{figure}"]) {
    assert.ok(result.figure.inside.text.includes(source), `${source} was not shown with the caret inside the figure's caption`);
  }
  assert.equal(result.figure.inside.images, 0, ".cm-lv-image stayed with the caret inside the figure");
  assert.equal(result.figure.hadImage, true, "there was no .cm-lv-image to click");
  assert.ok(result.figure.clicked.text.includes("\\begin{figure}"), "a click on the figure image did not show its source");
  console.log("visual-markup-browser: a figure folds into an image, shows its source with the caret inside, and after a click");

  // Check 6e: the kitchen-sink paper draws each construct, hides its source, and
  // shows the TikZ source after a click on its chip.
  const sinkBody = result.sink.text.replace(/\s+/g, " ");
  assert.equal(result.sink.katexShown, true, "KaTeX did not typeset $x \\in \\R$ in sink.tex within five seconds");
  for (const drawn of ["a", "b", "c", "–", "—", "“q”", "é", "ö", "…", "LaTeX", "©", "site", "“hi”", "3 m/s", "12 345", "ML", "numpy", "Lemma 1 (Key).", "Proof.", "∎", "Abstract", "Table of contents", "L’homme dit", "«oui»", "shown"]) {
    assert.ok(sinkBody.includes(drawn), `sink.tex does not draw '${drawn}'; visible text: ${sinkBody}`);
  }
  for (const source of ["\\textbf", "\\bfseries", "\\small", "\\verb", "\\'", "\\ldots", "\\href", "\\footnote", "\\enquote", "\\SI", "\\num", "\\gls", "\\pkg", "\\begin{lemma}", "\\begin{tikzpicture}", "\\draw", "\\tableofcontents", "\\texorpdfstring", "pdf alt", "\\og"]) {
    assert.equal(sinkBody.includes(source), false, `${source} is still visible in sink.tex with the caret on its last body line`);
  }
  assert.ok(result.sink.verbs.includes("x_1"), `the \\verb|x_1| did not draw as .cm-lp-verb 'x_1'; verb texts: ${JSON.stringify(result.sink.verbs)}`);
  assert.equal(result.sink.glyphWidths.length, 4, `the apostrophe, tie and French quotes did not draw as .cm-lv-glyph; markers: ${JSON.stringify(result.sink.markerTexts)}`);
  assert.ok(result.sink.glyphWidths.every((width) => width < 0.8), `a drawn glyph is wider than its character (in em): ${JSON.stringify(result.sink.glyphWidths)}`);
  assert.ok(result.sink.footnotes > 0, "the \\footnote did not draw as a .cm-lp-footnote");
  assert.equal(result.sink.tikzChip, true, `the tikzpicture did not fold into a pill; chips: ${JSON.stringify(result.sink.chips)}`);
  assert.ok(result.sink.links > 0, "the \\href did not draw as a .cm-lp-link");
  assert.equal(result.sink.cites, 3, `expected 3 .cm-lp-cite elements for the three citation commands, found ${result.sink.cites}`);
  assert.equal(result.sink.katexErrors, 0, "sink.tex has KaTeX parse errors (.katex-error inside .cm-editor)");
  assert.ok(result.sink.tikzClicked.includes("\\begin{tikzpicture}"), "a click on the TikZ chip did not show \\begin{tikzpicture}");
  console.log("visual-markup-browser: sink.tex draws its text commands, environments and citations, and a click on the TikZ chip shows its source");
  assert.deepEqual(result.offset.misses, [], `clicks at the middle of drawn lines landed on other lines: ${JSON.stringify(result.offset.misses)}`);
  console.log("visual-markup-browser: a click at the middle of each drawn line in multi-line blocks lands in that line");
  for (const source of ["\\begin{theorem}", "First line.", "\\end{theorem}"]) {
    assert.ok(result.offset.inBlock.includes(source), `with the caret inside the theorem, '${source}' is not shown as source`);
  }
  assert.equal(result.offset.envheads, 0, "with the caret inside the theorem, its header is still drawn");
  console.log("visual-markup-browser: a caret inside an environment shows the whole environment as source");

  // Check 6f: sink.typ draws its markup, links, footnote, swatch and references,
  // folds its setup lines into one chip, hides its source, and shows the setup
  // source after a click on the chip.
  const typstBody = result.typstSink.text.replace(/\s+/g, " ");
  for (const drawn of ["strong and emph", "warm", "—", "→", "🚀", "–", "…", "site", "Figure 1", "Table of contents", "Page break", "Bibliography", "Placeholder text", "#my-func"]) {
    assert.ok(typstBody.includes(drawn), `sink.typ does not draw '${drawn}'; visible text: ${typstBody}`);
  }
  for (const source of ["*strong*", "_emph_", "#underline", "#text(", "#link(", "#footnote", "#sym.", "#emoji.", "#set page", "#lorem", "#pagebreak"]) {
    assert.equal(typstBody.includes(source), false, `${source} is still visible in sink.typ with the caret on its last body line`);
  }
  assert.equal(result.typstSink.hidden.filter((text) => text.startsWith("Setup:")).length, 1, `expected one hidden chip starting with 'Setup:'; chips: ${JSON.stringify(result.typstSink.hidden)}`);
  assert.equal(result.typstSink.setupChip, true, "no .cm-lp-hidden chip starts with 'Setup:'");
  assert.ok(result.typstSink.footnotes > 0, "the #footnote did not draw as a .cm-lp-footnote");
  assert.ok(result.typstSink.links > 0, "the #link did not draw as a .cm-lp-link");
  assert.ok(result.typstSink.swatches > 0, "the #rgb did not draw as a .cm-lp-swatch");
  assert.equal(result.typstSink.katexErrors, 0, "sink.typ has KaTeX parse errors (.katex-error inside .cm-editor)");
  assert.ok(result.typstSink.warmStyle !== null && /color/.test(result.typstSink.warmStyle), `the #text(fill: red) text has no inline colour style; style: ${JSON.stringify(result.typstSink.warmStyle)}`);
  assert.ok(result.typstSink.clicked.includes("#set page"), "a click on the Setup chip did not show #set page");
  console.log("visual-markup-browser: sink.typ draws its markup, links, footnote, swatch and references, folds its setup lines, and a click on the setup chip shows its source");

  // Check 6g: Typst code fences. With the caret away from the block, the fence
  // markers are hidden and the code shows; sink.typ numbers its headings.
  assert.equal(result.typstFence.text.includes("```"), false, "the ``` fence markers are still visible in sink.typ with the caret away from the block");
  assert.ok(result.typstFence.text.includes("fn main() {}"), `the code of the fence is not visible in sink.typ; visible text: ${result.typstFence.text}`);
  assert.ok(result.typstFence.headnums.length >= 1, `sink.typ has no .cm-lp-headnum with #set heading(numbering: "1."); headnums: ${JSON.stringify(result.typstFence.headnums)}`);
  console.log("visual-markup-browser: sink.typ hides its code fence markers, shows its code, and numbers its headings");

  // Check 6h: the Vim and plain-key runs. Each starts with the caret on the text
  // line above the display equation, and the equation is drawn there.
  assert.equal(vimKeys.mounted.vimReady, true, "Vim did not load within five seconds");
  assert.equal(vimKeys.after.vimMode, true, "the Vim status panel or mode class is gone after the j key");
  for (const [label, run] of [["ArrowDown", plainKeys], ["j in Vim", vimKeys]]) {
    assert.equal(run.before.line, 3, `the caret was not on the text line above the equation before ${label}; line ${run.before.line}`);
    assert.ok(run.before.displays > 0, `no display equation was drawn before ${label}`);
    assert.equal(run.before.text.includes("\\begin{equation}"), false, `the equation source was visible before ${label}, with the caret above it`);
    assert.equal(run.after.line, 4, `${label} did not move the caret onto the equation's first line; line ${run.after.line}`);
    assert.ok(run.after.text.includes("\\begin{equation}"), `${label} did not reveal the equation's source; visible text: ${run.after.text}`);
    assert.equal(run.after.inside, true, `the caret is not inside the equation's range after ${label}`);
  }
  console.log("visual-markup-browser: one step down onto a display equation reveals its source, with plain keys and in Vim");

  // Check 7: timing, printed only.
  console.log(`visual-markup-browser timing (informational, not asserted, ${result.timing.lines} lines): ` +
    `layer on median ${result.timing.on.median} ms, max ${result.timing.on.max} ms; ` +
    `layer off median ${result.timing.off.median} ms, max ${result.timing.off.max} ms`);
  console.log(`visual-markup-browser caret moves (informational, not asserted, ${result.caret.lines} lines, layer on): ` +
    `median ${result.caret.median} ms, max ${result.caret.max} ms`);
  console.log(`visual-markup-browser 15,000-line typing (informational, not asserted, ${result.hugeTiming.lines} lines): ` +
    `layer on median ${result.hugeTiming.on.median} ms, max ${result.hugeTiming.on.max} ms; ` +
    `layer off median ${result.hugeTiming.off.median} ms, max ${result.hugeTiming.off.max} ms`);

  assert.equal(errors.length, 0, ["Browser errors or exceptions during the visual markup check:", ...errors].join("\n"));
  console.log("visual-markup-browser: no browser errors or exceptions");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
