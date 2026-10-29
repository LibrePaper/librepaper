// A browser-level check for the visual markup spike: the layer that draws
// LaTeX and Typst sources in their rendered form while they stay plain text.
//
// The Editor is mounted with the layer on, and every check reads the DOM and
// the shared document only. Formulas are typeset by KaTeX, concealed commands
// drop out of the visible text, a caret inside a formula shows its source,
// edits reach the shared text, the visual switch turns the layer off and on
// in place, Typst is drawn too, and Markdown is left alone. Timings for long
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
// A whole LaTeX paper: a preamble, math environments, labels and a bibliography.
const fullSample = [
  "\\documentclass[11pt]{article}",
  "\\usepackage[T1]{fontenc}",
  "\\usepackage{amsmath}",
  "\\usepackage{booktabs,graphicx}",
  "\\begin{document}",
  "\\section{Results}",
  "The estimate is in Equation \\ref{eq:se}.",
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
  "See Equation~\\eqref{eq:a} and Section~\\ref{sec:m} and \\ref{sec:none}.",
  "\\begin{equation}",
  "a = b \\label{eq:a}",
  "\\end{equation}",
  "\\end{document}",
].join("\n");
const introSample = "\\section{Intro}\nText.";

// Runs in the page. It refers only to names the generated entry defines
// (component, session, ids, samples, tick, EditorView), so its source is
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

  // Check 5: Typst is drawn.
  component.$set({ file: ids.typ, format: "typst" });
  await settle();
  const typst = {
    katex: await poll(mathKatexCount),
    text: visibleText(),
    marks: markCount(),
  };

  // Check 6: Markdown is left alone.
  component.$set({ file: ids.md, format: "markdown" });
  await settle();
  const markdown = { marks: markCount(), text: visibleText() };

  // Check 6b: a LaTeX paper. The preamble folds into one block while the caret
  // is in the body, labels and the bibliography are hidden, math environments
  // are typeset, and a caret in the preamble shows the source again.
  component.$set({ visual: true, file: ids.full, format: "latex" });
  await settle();
  const fullKatexShown = await poll(mathKatexCount);
  view().dispatch({ selection: { anchor: view().state.doc.toString().indexOf("\\section{Results}") + 2 } });
  await settle();
  const fullPreambles = document.querySelectorAll(".cm-editor .cm-lp-preamble");
  const fullBody = {
    katexShown: fullKatexShown,
    preambleCount: fullPreambles.length,
    preambleText: fullPreambles.length ? fullPreambles[0].textContent : "",
    katex: mathKatexCount(),
    katexErrors: document.querySelectorAll(".cm-editor .katex-error").length,
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
    off,
    back,
    sameView,
    typst,
    markdown,
    full: { body: fullBody, preamble: fullPreamble, end: fullEnd },
    refs: refsCheck,
    timing: { lines: timingOn.lines, on: { median: timingOn.median, max: timingOn.max }, off: { median: timingOff.median, max: timingOff.max } },
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
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `const samples = ${JSON.stringify({ tex: texSample, typst: typstSample, markdown: markdownSample, full: fullSample, long: longSample, huge: hugeSample, refs: refsSample, intro: introSample })};`,
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
  `  full: session.addText("full.tex", samples.full),`,
  `  big: session.addText("big.tex", samples.long),`,
  `  bigOff: session.addText("big-off.tex", samples.long),`,
  `  huge: session.addText("huge.tex", samples.huge),`,
  `  hugeOff: session.addText("huge-off.tex", samples.huge),`,
  `  refs: session.addText("refs.tex", samples.refs),`,
  `  intro: session.addText("sections/intro.tex", samples.intro),`,
  `};`,
  `const component = createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "latex", file: ids.tex, visual: true },`,
  `});`,
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

  // Check 5: Typst is drawn.
  assert.equal(result.typst.katex, true, "a Typst formula was not drawn as KaTeX inside .cm-lv-math");
  console.log("visual-markup-browser: a Typst formula is drawn");

  // Check 6: Markdown is left alone.
  assert.equal(result.markdown.marks, 0, "Markdown was drawn by the visual layer");
  console.log("visual-markup-browser: Markdown is left alone");

  // Check 6b: a LaTeX preamble folds, labels and the bibliography are hidden,
  // math environments are typeset, and the preamble source shows with the caret in it.
  assert.equal(result.full.body.katexShown, true, "KaTeX did not typeset the math environments in full.tex within five seconds");
  assert.equal(result.full.body.preambleCount, 1, "the LaTeX preamble did not fold into exactly one .cm-lp-preamble block with the caret in the body");
  assert.match(result.full.body.preambleText, /Preamble: article, 4 packages/, "the preamble block did not read as 'Preamble: article, 4 packages'");
  const hiddenInBody = [
    "\\documentclass", "\\usepackage", "\\label", "sec:results",
    "\\bibliographystyle", "\\bibliography", "references", "\\newpage",
    "\\begin{document}", "\\end{document}", "\\begin{equation}", "\\begin{align}",
  ];
  for (const source of hiddenInBody) {
    assert.equal(result.full.body.text.includes(source), false, `${source} is still visible in full.tex with the caret in the body`);
  }
  // The \ref keeps its key on show; only the \label's copy must be gone.
  assert.equal(result.full.body.text.split("eq:se").length - 1, 1, "eq:se should show once, in the \\ref, and not in the hidden \\label");
  assert.ok(result.full.body.katex >= 4, `expected at least 4 KaTeX formulas inside .cm-lv-math in full.tex, found ${result.full.body.katex}`);
  assert.equal(result.full.body.katexErrors, 0, "full.tex has KaTeX parse errors (.katex-error inside .cm-editor)");
  assert.equal(result.full.preamble.preambleCount, 0, "the preamble block stayed when the caret moved into the preamble");
  assert.ok(result.full.preamble.text.includes("\\documentclass"), "the preamble source did not show with the caret in the preamble");
  assert.ok(result.full.end.text.includes("\\end{document}"), "\\end{document} did not show with the caret on that line");
  console.log(`visual-markup-browser: full.tex folds its preamble, hides labels and the bibliography, and typesets ${result.full.body.katex} math environments`);

  // Check 6c: title, include, references and the labelled equation in refs.tex.
  assert.equal(result.refs.title.main, "A Study", "the \\title text was not drawn in .cm-lp-title-main");
  assert.ok(result.refs.title.author.includes("Ada") && result.refs.title.author.includes("Bob"), "the \\author names were not drawn in .cm-lp-title-author");
  assert.equal(result.refs.title.date, "2020", "the \\date text was not drawn in .cm-lp-title-date");
  assert.ok(result.refs.include.text.includes("sections/intro"), "the \\input name was not drawn in .cm-lp-include");
  assert.equal(result.refs.include.open, true, "the \\input widget has no .cm-lp-include-open button");
  assert.ok(result.refs.refTexts.includes("(1)"), `the \\eqref did not show (1); .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  assert.ok(result.refs.refTexts.includes("2"), `the \\ref to Method did not show 2; .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  assert.ok(result.refs.refTexts.includes("??"), `the \\ref to a missing label did not show ??; .cm-lp-ref texts: ${JSON.stringify(result.refs.refTexts)}`);
  for (const source of ["~", "\\ref", "\\eqref", "\\maketitle", "\\input"]) {
    assert.equal(result.refs.text.includes(source), false, `${source} is still visible in refs.tex with the caret in the body`);
  }
  assert.ok(result.refs.katexTexts.some((text) => text.includes("(1)")), "the labelled equation did not show its number (1) through KaTeX");
  console.log(`visual-markup-browser: refs.tex draws its title, include, references ${JSON.stringify(result.refs.refTexts)} and equation number`);

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
