// A browser-level check for the visual mode formatting toolbar: bold, italic,
// underline, headings and lists, in each of the three markup formats.
//
// A plain CodeMirror view is mounted with the visual layer on, and the
// formatting actions are applied to it through applyFormat and read back with
// formatState. Every check reads the shared document text and the reported
// state only. Each format is run in turn, one log line per format, and every
// failing expectation is collected and reported together at the end.

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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-visual-format-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
const katexDist = join(root, "web/node_modules/katex/dist");
const port = 19000 + Math.floor(Math.random() * 1000);

// Four plain lines shared by every format: a word to wrap, a line to make a
// heading, and two lines to make a list.
const plainLines = ["Some words here.", "A plain line.", "First item", "Second item"];
const plainDoc = plainLines.join("\n");
const word = "words";

// The markup each format writes. Italic accepts either LaTeX command.
const formats = {
  latex: {
    wrap: {
      bold: [(text) => `\\textbf{${text}}`],
      italic: [(text) => `\\textit{${text}}`, (text) => `\\emph{${text}}`],
      underline: [(text) => `\\underline{${text}}`],
    },
    heading: (level, text) => `${["\\section{", "\\subsection{", "\\subsubsection{"][level - 1]}${text}}`,
    bullet: /^\\item\s+(.+)$/,
    number: /^\\item\s+(.+)$/,
    environment: { bullet: "\\begin{itemize}", number: "\\begin{enumerate}" },
  },
  typst: {
    wrap: {
      bold: [(text) => `*${text}*`],
      italic: [(text) => `_${text}_`],
      underline: [(text) => `#underline[${text}]`],
    },
    heading: (level, text) => `${"=".repeat(level)} ${text}`,
    bullet: /^- (.+)$/,
    number: /^\+ (.+)$/,
    environment: {},
  },
  markdown: {
    wrap: {
      bold: [(text) => `**${text}**`],
      italic: [(text) => `*${text}*`],
      underline: [(text) => `[${text}]{.underline}`],
    },
    heading: (level, text) => `${"#".repeat(level)} ${text}`,
    bullet: /^- (.+)$/,
    number: /^\d+\. (.+)$/,
    environment: {},
  },
};

// Runs in the page. It refers only to names the generated entry defines
// (EditorView, EditorState, visualMarkup, applyFormat, formatState), so its
// source is copied into the entry as it stands. It drives the editor and
// returns one snapshot per step; the Node side decides what each one must say.
async function formatCheck(format, sample) {
  let view = null;
  const mount = () => {
    view = new EditorView({
      state: EditorState.create({ doc: sample, extensions: [visualMarkup(format)] }),
      parent: document.body,
    });
  };
  const find = (text) => {
    const at = view.state.doc.toString().indexOf(text);
    if (at < 0) throw new Error(`"${text}" is not in the document`);
    return at;
  };
  const select = (text) => {
    const at = find(text);
    view.dispatch({ selection: { anchor: at, head: at + text.length } });
  };
  const caretIn = (text) => {
    view.dispatch({ selection: { anchor: find(text) + 2 } });
  };
  const selectBetween = (first, last) => {
    const from = view.state.doc.lineAt(find(first)).from;
    const to = view.state.doc.lineAt(find(last)).to;
    view.dispatch({ selection: { anchor: from, head: to } });
  };
  const snapshots = [];
  const step = (label, action) => {
    let error = null;
    try {
      action();
    } catch (failure) {
      error = String(failure?.stack ?? failure);
    }
    const state = view.state;
    snapshots.push({
      label,
      error,
      doc: state.doc.toString(),
      head: state.selection.main.head,
      format: formatState(view, format),
    });
  };

  mount();
  step("start", () => {});
  step("bold on", () => { select("words"); applyFormat(view, format, "bold"); });
  step("bold off", () => applyFormat(view, format, "bold"));
  step("italic on", () => { select("words"); applyFormat(view, format, "italic"); });
  step("italic off", () => applyFormat(view, format, "italic"));
  step("underline on", () => { select("words"); applyFormat(view, format, "underline"); });
  step("underline off", () => applyFormat(view, format, "underline"));
  step("heading 2 on", () => { caretIn("A plain line."); applyFormat(view, format, "h2"); });
  step("heading 2 off", () => applyFormat(view, format, "h2"));
  step("bullets on", () => { selectBetween("First item", "Second item"); applyFormat(view, format, "bullets"); });
  step("numbers on", () => { selectBetween("First item", "Second item"); applyFormat(view, format, "numbers"); });
  step("numbers off", () => { selectBetween("First item", "Second item"); applyFormat(view, format, "numbers"); });
  view.destroy();
  return snapshots;
}

const source = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { EditorState } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/state/dist/index.js"))};`,
  `import { visualMarkup } from ${JSON.stringify(join(root, "web/src/lib/visual-markup/index.js"))};`,
  `import { applyFormat, formatState } from ${JSON.stringify(join(root, "web/src/lib/visual-markup/formatting.js"))};`,
  `window.formatCheck = ${formatCheck.toString()};`,
  `window.formatCheckReady = true;`,
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
      lib: { entry, formats: ["es"], fileName: () => "visual-format-check.js", cssFileName: "visual-format-check" },
    },
  });

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
    response.end('<!doctype html><body><link rel="stylesheet" href="/visual-format-check.css"><script type="module" src="/visual-format-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local visual format server did not start");

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
    ready = await evaluate("Boolean(window.formatCheckReady)");
    if (ready) break;
    await wait(100);
  }
  assert.equal(ready, true, "the formatting check did not load in Chromium");

  // Checks for one format, judged against the snapshots the page returned.
  // Each check that fails adds one message to the list for that format.
  const judge = (format, snapshots) => {
    const spec = formats[format];
    const problems = [];
    let checks = 0;
    const expect = (ok, message) => {
      checks++;
      if (!ok) problems.push(message);
    };
    const at = Object.fromEntries(snapshots.map((snapshot) => [snapshot.label, snapshot]));
    for (const snapshot of snapshots) {
      expect(snapshot.error === null, `${snapshot.label}: the action threw: ${snapshot.error}`);
    }

    const replaceLine = (index, text) => plainLines.map((line, position) => (position === index ? text : line)).join("\n");
    const wrapped = (kind, index) => spec.wrap[kind].map((wrap) => replaceLine(index, plainLines[index].replace(word, wrap(word))));

    // The caret must sit inside the wrapped word, from its first letter to its end.
    const caretInWord = (snapshot) => {
      const start = snapshot.doc.indexOf(word);
      return start >= 0 && snapshot.head >= start && snapshot.head <= start + word.length;
    };

    expect(at.start.doc === plainDoc, `start: the sample changed before any action; found ${JSON.stringify(at.start.doc)}`);
    expect(at.start.format.bold === false && at.start.format.italic === false && at.start.format.underline === false
      && at.start.format.heading === 0 && at.start.format.list === null,
    `start: a plain sample reported formatting ${JSON.stringify(at.start.format)}`);

    for (const kind of ["bold", "italic", "underline"]) {
      const on = at[`${kind} on`];
      const off = at[`${kind} off`];
      const expected = wrapped(kind, 0);
      expect(expected.includes(on.doc),
        `${kind} on: expected one of ${JSON.stringify(expected)}, found ${JSON.stringify(on.doc)}`);
      expect(on.format[kind] === true, `${kind} on: formatState did not report ${kind}; found ${JSON.stringify(on.format)}`);
      expect(caretInWord(on), `${kind} on: the caret (${on.head}) is not inside the word in ${JSON.stringify(on.doc)}`);
      expect(off.doc === plainDoc, `${kind} off: the markup was not removed; expected ${JSON.stringify(plainDoc)}, found ${JSON.stringify(off.doc)}`);
      expect(off.format[kind] === false, `${kind} off: formatState still reports ${kind}; found ${JSON.stringify(off.format)}`);
    }

    const heading = at["heading 2 on"];
    expect(heading.doc === replaceLine(1, spec.heading(2, "A plain line.")),
      `heading 2 on: expected the heading line ${JSON.stringify(spec.heading(2, "A plain line."))}, found ${JSON.stringify(heading.doc)}`);
    expect(heading.format.heading === 2, `heading 2 on: formatState did not report heading 2; found ${JSON.stringify(heading.format)}`);
    expect(at["heading 2 off"].doc === plainDoc, `heading 2 off: the heading was not removed; found ${JSON.stringify(at["heading 2 off"].doc)}`);
    expect(at["heading 2 off"].format.heading === 0, `heading 2 off: formatState reports heading ${at["heading 2 off"].format.heading}`);

    // Two list lines, each made into an item, with the other lines left alone.
    const itemsOk = (doc, regex) => {
      const all = doc.split("\n");
      const index = all.findIndex((line) => line.includes("First item"));
      if (index < 0 || index + 1 >= all.length) return `the lines "First item" and "Second item" were not both kept; found ${JSON.stringify(doc)}`;
      const first = all[index].match(regex);
      const second = all[index + 1].match(regex);
      if (!first || first[1].trim() !== "First item") return `the line ${JSON.stringify(all[index])} is not a list item`;
      if (!second || second[1].trim() !== "Second item") return `the line ${JSON.stringify(all[index + 1])} is not a list item`;
      if (!all.includes(plainLines[0]) || !all.includes(plainLines[1])) return `the lines outside the list changed; found ${JSON.stringify(doc)}`;
      return null;
    };
    const bullets = at["bullets on"];
    const bulletProblem = itemsOk(bullets.doc, spec.bullet);
    expect(bulletProblem === null, `bullets on: ${bulletProblem}`);
    if (spec.environment.bullet) {
      expect(bullets.doc.includes(spec.environment.bullet), `bullets on: ${spec.environment.bullet} is missing; found ${JSON.stringify(bullets.doc)}`);
    }
    expect(bullets.format.list === "bullet", `bullets on: formatState did not report a bullet list; found ${JSON.stringify(bullets.format)}`);

    const numbers = at["numbers on"];
    const numberProblem = itemsOk(numbers.doc, spec.number);
    expect(numberProblem === null, `numbers on: ${numberProblem}`);
    if (spec.environment.number) {
      expect(numbers.doc.includes(spec.environment.number), `numbers on: ${spec.environment.number} is missing; found ${JSON.stringify(numbers.doc)}`);
    }
    expect(numbers.format.list === "number", `numbers on: formatState did not report a numbered list; found ${JSON.stringify(numbers.format)}`);

    const plainAgain = at["numbers off"];
    expect(plainAgain.doc === plainDoc, `numbers off: the lines were not made plain again; expected ${JSON.stringify(plainDoc)}, found ${JSON.stringify(plainAgain.doc)}`);
    expect(plainAgain.format.list === null, `numbers off: formatState still reports a list; found ${JSON.stringify(plainAgain.format)}`);

    return { checks, problems };
  };

  let failures = 0;
  for (const format of Object.keys(formats)) {
    const snapshots = await evaluate(`formatCheck(${JSON.stringify(format)}, ${JSON.stringify(plainDoc)})`);
    const { checks, problems } = judge(format, snapshots);
    if (problems.length) {
      failures += problems.length;
      console.log(`visual-format-browser: ${format} FAILED ${problems.length} of ${checks} checks`);
      for (const problem of problems) console.log(`  - ${problem}`);
    } else {
      console.log(`visual-format-browser: ${format} passed ${checks} checks (bold, italic, underline, headings and lists)`);
    }
  }

  assert.equal(failures, 0, "some visual formatting checks failed; see the lines above");
  assert.equal(errors.length, 0, ["Browser errors or exceptions during the visual formatting check:", ...errors].join("\n"));
  console.log("visual-format-browser: no browser errors or exceptions");
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
