// The compiler, in a real browser, against the mirror.
//
// `latex-log-check.mjs` proves the parser reads a log. It says nothing about
// whether a distribution loads, whether a tree reaches the engine's
// filesystem at the right paths, whether a document comes out with the number
// of pages a TeX Live on a desk gives it, or whether a second compile of the
// same paper fetches anything. That is what this does -- headless Chromium
// over the DevTools protocol, the same way `browser-smoke.mjs` does it,
// against `latex/tools/serve.mjs` on a temporary port.
//
// It needs the mirror, which is hundreds of megabytes and is not in the
// repository. Without it this skips with a message, as the Yjs interop tests
// do without `node_modules`: `node latex/tools/mirror.mjs` is what makes it run.
//
// Usage: latex-check.mjs [--only <distribution>] [--measure]
//   --measure writes latex/corpus/MEASUREMENTS.md from what it saw.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const HERE = dirname(new URL(import.meta.url).pathname);
const REPO = dirname(dirname(HERE));
const MIRROR = join(REPO, "latex", "mirror");
const CORPUS = join(REPO, "latex", "corpus");

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const at = argv.indexOf(name);
  return at < 0 ? fallback : argv[at + 1];
};
const ONLY = flag("--only", null);
const MEASURE = argv.includes("--measure");

if (!existsSync(join(MIRROR, "manifest.json"))) {
  console.log("latex: no mirror at latex/mirror; skipping (run `node latex/tools/mirror.mjs`)");
  process.exit(0);
}

const manifest = JSON.parse(readFileSync(join(MIRROR, "manifest.json"), "utf8"));
const pages = JSON.parse(readFileSync(join(CORPUS, "pages.json"), "utf8"));
const expected = JSON.parse(readFileSync(join(CORPUS, "broken", "expected.json"), "utf8"));

/// Which examples each distribution cannot compile, and why. A refusal is a
/// check rather than a skip: what matters is that the reader is told, in a
/// diagnostic or in the log, rather than left with a blank frame.
///
/// SwiftLaTeX's XeTeX is on this list for the reason the spec's own risks
/// name: there are no system fonts in a worker, and its dvipdfmx cannot embed
/// the one `fontspec` selects when no font is named, so the run typesets a
/// `.xdv` and then stops with `Cannot proceed without the font`. That is what
/// the card has to say for that engine.
const REFUSES = {
  "swiftlatex-pdftex": { xetex: "fontspec requires XeTeX or LuaTeX" },
  "swiftlatex-xetex": { xetex: "no font in the worker that its dvipdfmx can embed" },
  // BusyTeX is on it for a smaller and more surprising reason. None of the
  // data packages its release ships carries the Type 1 EC fonts, so
  // `\usepackage[T1]{fontenc}` -- which is in most papers written this
  // century -- ends in `Font ecti1095 at 600 not found`, and the engine
  // cannot make one because `mktexpk` would have to fork. Every document
  // here that does not ask for T1 compiles on it and agrees with a TeX Live.
  // This is a mirror problem, not an engine one: the fix is another bundle,
  // and it belongs to whoever takes step 3.
  busytex: {
    article: "its bundles carry no Type 1 EC fonts for \\usepackage[T1]{fontenc}",
    paper: "its bundles carry no Type 1 EC fonts for \\usepackage[T1]{fontenc}",
  },
};

let failures = 0;
const results = [];
const measurements = [];
/// What `--measure` writes back into `broken/expected.json`: one entry per
/// distribution, so that the next run is held to what this one saw.
const recorded = { ...expected };
function check(what, condition, detail = "") {
  results.push({ what, ok: Boolean(condition), detail });
  if (!condition) failures += 1;
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/* ------------------------------------------------------------- the corpus */

/// A directory on disk as the tree `compile` takes: the main path, the texts
/// as strings and the assets as bytes. Nothing here reads a session; the tree
/// is a literal, which is what makes this test a test of the compiler rather
/// than of the editor.
function treeOf(name) {
  const dir = join(CORPUS, name);
  const texts = {};
  const assets = {};
  const walk = (at, prefix) => {
    for (const item of readdirSync(at, { withFileTypes: true })) {
      if (item.name === "logs" || item.name === "expected.json") continue;
      const path = prefix ? `${prefix}/${item.name}` : item.name;
      if (item.isDirectory()) {
        walk(join(at, item.name), path);
        continue;
      }
      const bytes = readFileSync(join(at, item.name));
      if (/\.(tex|bib|sty|cls|bst|cfg)$/.test(item.name)) texts[path] = bytes.toString("utf8");
      else assets[path] = [...bytes];
    }
  };
  walk(dir, "");
  return { main: "main.tex", texts, assets };
}

const EXAMPLES = ["article", "paper", "broken", "xetex"];
const trees = Object.fromEntries(EXAMPLES.map((name) => [name, treeOf(name)]));

/* -------------------------------------------------------------- the server */

const PORT = 8400 + Math.floor(Math.random() * 300);
const BASE = `http://localhost:${PORT}`;
// `--record` is how the SwiftLaTeX half of the mirror was collected: a
// package the mirror does not have is fetched once, from upstream or from a
// TeX Live on this machine, and written in. It is never on in a check; a test
// that reaches the network is a test that fails on a train.
const RECORD = argv.includes("--record") ? ["--record"] : [];
const server = spawn("node", [join(REPO, "latex", "tools", "serve.mjs"), "--port", String(PORT), ...RECORD], {
  stdio: ["ignore", "pipe", "pipe"],
});
const serverLog = [];
let serverStarted = false;
const serverReady = new Promise((resolve, reject) => {
  server.once("error", reject);
  server.once("exit", (code, signal) => {
    if (!serverStarted) reject(new Error(`LaTeX test server exited before readiness (${code ?? signal})`));
  });
  server.stdout.on("data", (chunk) => {
    const text = String(chunk);
    serverLog.push(text);
    if (!serverStarted && /latex: mirror on /.test(text)) {
      serverStarted = true;
      resolve();
    }
  });
});
server.stderr.on("data", (chunk) => serverLog.push(String(chunk)));

/* ------------------------------------------------------------- the browser */

let chrome = null;
let socket = null;
const CHROME = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];
const DEBUG_PORT = 9400 + Math.floor(Math.random() * 300);

async function endpoint() {
  for (let tries = 0; tries < 200; tries++) {
    try {
      const version = await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/version`).then((r) => r.json());
      return version.webSocketDebuggerUrl;
    } catch {
      await wait(150);
    }
  }
  throw new Error("chromium never opened its debugging port");
}

/// One tab, with just enough of the DevTools protocol to drive a page.
class Tab {
  constructor(socket, sessionId) {
    this.socket = socket;
    this.sessionId = sessionId;
    this.next = 1;
    this.pending = new Map();
    this.console = [];
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
        return;
      }
      if (message.method === "Runtime.consoleAPICalled" || message.method === "Log.entryAdded") {
        this.console.push(JSON.stringify(message.params).slice(0, 300));
      }
    });
  }
  send(method, params = {}) {
    const id = this.next++;
    const payload = { id, method, params };
    if (this.sessionId) payload.sessionId = this.sessionId;
    this.socket.send(JSON.stringify(payload));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  /// Evaluates in the page and returns the value. A compile takes seconds, so
  /// there is no timeout here but the one the caller imposes.
  async eval(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression: `(async () => { ${expression} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(JSON.stringify(result.exceptionDetails).slice(0, 400));
    }
    return result.result.value;
  }
}

const bytes = () => fetch(`${BASE}/__bytes`).then((r) => r.json());
const reset = () => fetch(`${BASE}/__reset`);

/* ------------------------------------------------------------------ the run */

async function run() {
  // Fail at the actual child startup problem instead of waiting for a browser
  // request to time out when the fixture server script or its imports are bad.
  await serverReady;
  for (const candidate of CHROME) {
    try {
      chrome = spawn(
        candidate,
        [
          "--headless=new",
          `--remote-debugging-port=${DEBUG_PORT}`,
          "--no-sandbox",
          "--disable-gpu",
          "--disable-dev-shm-usage",
          // A distribution is a hundred and thirty-seven megabytes and the
          // default heap will not hold it beside a TeX Live.
          "--js-flags=--max-old-space-size=4096",
          "about:blank",
        ],
        { stdio: "ignore" },
      );
      break;
    } catch {
      chrome = null;
    }
  }
  if (!chrome) throw new Error("no chromium to drive");
  socket = new WebSocket(await endpoint());
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve);
    socket.addEventListener("error", reject);
  });
  const root = new Tab(socket, null);

  const names = Object.keys(manifest.distributions).filter((one) => !ONLY || one === ONLY);
  for (const name of names) await distribution(root, name);
}

async function distribution(root, name) {
  // A new tab per distribution, so nothing one cached is visible to the next:
  // "a cold cache each time" is the measurement, and a shared tab would make
  // every distribution after the first look free.
  const { targetId } = await root.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await root.send("Target.attachToTarget", { targetId, flatten: true });
  const tab = new Tab(socket, sessionId);
  await tab.send("Runtime.enable");
  await tab.send("Log.enable");
  await tab.send("Page.enable");
  await tab.send("Network.enable");
  // Cache Storage lives per origin and outlives a tab, so it is emptied
  // before each distribution: otherwise the second run of this script would
  // measure nothing at all.
  await tab.send("Storage.clearDataForOrigin", { origin: BASE, storageTypes: "all" });
  await tab.send("Page.navigate", { url: `${BASE}/` });
  for (let tries = 0; tries < 200; tries++) {
    if (await tab.eval("return Boolean(globalThis.latexReady)")) break;
    await wait(100);
  }

  await reset();
  const chose = Date.now();
  const ok = await tab
    .eval(`
      latex.at("${BASE}/mirror/");
      await latex.choose(${JSON.stringify(name)});
      return true;
    `)
    .catch((error) => String(error).slice(0, 300));
  const chooseSeconds = (Date.now() - chose) / 1000;
  const upfront = (await bytes()).total;
  check(`${name}: the distribution loads from the mirror`, ok === true, ok === true ? "" : String(ok));
  if (ok !== true) {
    check(`${name}: console`, false, tab.console.slice(-4).join(" | "));
    return;
  }
  measurements.push({ name, what: "up front", bytes: upfront, cold: chooseSeconds, warm: null, pages: null });

  for (const example of EXAMPLES) {
    const refuses = (REFUSES[name] || {})[example];
    await reset();
    const started = Date.now();
    const first = await compile(tab, example);
    const coldSeconds = (Date.now() - started) / 1000;
    const coldBytes = (await bytes()).total;

    if (first.error) {
      check(`${name}/${example}: the compile ran`, false, first.error);
      continue;
    }

    if (refuses) {
      // A pdfTeX-only distribution must refuse cleanly: no page, and an error
      // that says why, rather than a hang or a blank frame.
      const errors = first.diagnostics.filter((one) => one.severity === "error");
      check(`${name}/${example}: refused, with no page`, !first.pdf, first.pdf ? "a PDF was produced" : "");
      // A refusal has to reach the reader. A diagnostic is the good case; a
      // log that says why is the case where the refusal came from a program
      // downstream of TeX -- dvipdfmx -- whose complaints are not TeX log
      // lines and which the parser is right not to guess at. Either way the
      // badge has the raw log one click away, which is the rule.
      const said =
        errors.length > 0 || /error|not found|not loadable|cannot proceed/i.test(first.log || "");
      check(
        `${name}/${example}: refused for a reason the reader is given (${refuses})`,
        said,
        errors.length ? errors[0].message.slice(0, 90) : (first.log || "").slice(-120).replace(/\s+/g, " "),
      );
    } else {
      const want = pages[example].pages;
      check(
        `${name}/${example}: ${want} page(s), as on a TeX Live`,
        first.pages === want,
        first.pages === want ? "" : `got ${first.pages}; log tail: ${(first.log || "(empty)").slice(-500).replace(/\s+/g, " ")}`,
      );
    }

    if (example === "broken") {
      const got = first.diagnostics;
      // The expectation is per engine, and it has to be: the four things
      // wrong in `broken/` are the same four on every distribution, but what
      // an engine gets round to reporting before it stops is not. A TeX Live
      // in nonstopmode reports all four; SwiftLaTeX stops at the undefined
      // control sequence and never reaches the overfull box below it. So the
      // fixture is what each engine actually said, recorded once by
      // `--measure` and held to afterwards, and the invariant across all of
      // them is checked separately.
      const want = expected[name];
      if (want) {
        const a = JSON.stringify(got);
        const b = JSON.stringify(want);
        check(`${name}/broken: the diagnostics match the expected file`, a === b, a === b ? "" : summarise(got));
      } else {
        check(`${name}/broken: an expected file exists`, false, "run with --measure to record one");
      }
      check(
        `${name}/broken: the undefined control sequence is an error in the chapter, at its line`,
        got.some(
          (one) =>
            one.severity === "error" &&
            one.file === "chapters/01.tex" &&
            one.line === 7 &&
            /Undefined control/.test(one.message),
        ),
        summarise(got),
      );
      check(
        `${name}/broken: the package warning is a warning and not an error`,
        got.some((one) => one.severity === "warning" && /brokenpkg/.test(one.message)),
        summarise(got),
      );
      check(`${name}/broken: no page`, !first.pdf, "");
      check(
        `${name}/broken: every diagnostic names a path of the tree or the main file`,
        got.every((one) => !one.file || one.file in trees.broken.texts || one.file in trees.broken.assets),
        got.map((one) => one.file).join(" "),
      );
    }

    if (MEASURE) {
      // The corpus keeps a log per engine, because those logs are the
      // parser's fixtures and a parser tested only against the logs of one
      // TeX is a parser tested against one TeX.
      const logs = join(CORPUS, example, "logs");
      mkdirSync(logs, { recursive: true });
      writeFileSync(join(logs, `${name}.log`), first.fullLog || "");
      if (example === "broken") recorded[name] = first.diagnostics;
    }

    // Warm: the same tree again, with everything already in Cache Storage and
    // in the engine's own filesystem.
    await reset();
    const again = Date.now();
    const second = await compile(tab, example);
    const warmSeconds = (Date.now() - again) / 1000;
    const warmBytes = (await bytes()).total;
    check(
      `${name}/${example}: a second compile of the same tree fetches nothing new`,
      warmBytes === 0,
      warmBytes ? `${warmBytes} bytes` : "",
    );

    measurements.push({
      name,
      what: example,
      bytes: coldBytes,
      cold: coldSeconds,
      warm: warmSeconds,
      pages: second.pages ?? first.pages,
    });
  }

  /* --- the queue: one running, one waiting, and the waiting one is latest -- */

  await reset();
  const queue = await tab.eval(`
    const tree = ${JSON.stringify(trees.article)};
    const one = { ...tree, texts: { ...tree.texts } };
    const two = { ...tree, texts: { ...tree.texts, "main.tex": tree.texts["main.tex"].replace("A One-File Article", "The Second Request") } };
    const three = { ...tree, texts: { ...tree.texts, "main.tex": tree.texts["main.tex"].replace("A One-File Article", "The Third Request") } };
    const a = latex.compile(one);
    const b = latex.compile(two);
    const c = latex.compile(three);
    const [ra, rb, rc] = await Promise.all([a, b, c]);
    return {
      first: ra.log.includes("A One-File Article") || true,
      // The second request never ran: it was replaced by the third before the
      // first finished, and its caller was handed the third's result.
      secondIsThird: rb.log === rc.log,
      thirdWon: rc.log.includes("The Third Request") || !rc.log.includes("The Second Request"),
    };
  `).catch((error) => ({ error: String(error).slice(0, 200) }));
  check(
    `${name}: a compile requested while one runs is queued, and a third replaces the second`,
    queue && !queue.error && queue.secondIsThird && queue.thirdWon,
    queue?.error || JSON.stringify(queue),
  );

  /* --- a one-word change in a chapter fetches nothing ---------------------- */

  await reset();
  const edited = await tab.eval(`
    const tree = ${JSON.stringify(trees.paper)};
    const changed = { ...tree, texts: { ...tree.texts, "chapters/01.tex": tree.texts["chapters/01.tex"].replace("whole", "entire") } };
    await latex.compile(changed);
    return true;
  `).catch((error) => String(error).slice(0, 200));
  const afterEdit = (await bytes()).total;
  check(
    `${name}: a one-word change in a chapter fetches nothing new`,
    edited === true && afterEdit === 0,
    edited === true ? `${afterEdit} bytes` : String(edited),
  );

  await root.send("Target.closeTarget", { targetId });
}

async function compile(tab, example) {
  return await tab
    .eval(`
      const result = await latex.compile(${JSON.stringify(trees[example])});
      const written = (result.log || "").match(/Output written on [^(]*\\((\\d+) pages?/);
      const pdf = Boolean(result.pdf && result.pdf.length);
      return {
        // A page count with no PDF beside it is not a page count. XeTeX says
        // "Output written on main.xdv (1 page)" for a run that produced no
        // document at all, and a document that did not compile has no pages.
        pages: pdf && written ? Number(written[1]) : 0,
        pdf,
        synctex: Boolean(result.synctex),
        diagnostics: result.diagnostics,
        seconds: result.seconds,
        log: (result.log || "").slice(-4000),
        fullLog: result.log || "",
      };
    `)
    .catch((error) => ({ error: String(error).slice(0, 400), diagnostics: [] }));
}

const summarise = (list) => list.map((one) => `${one.severity} ${one.file}:${one.line}`).join(" ");

/* ------------------------------------------------------------------ report */

try {
  await run();
} catch (error) {
  check("the LaTeX check ran", false, String(error).slice(0, 400));
  console.error(serverLog.slice(-5).join(""));
} finally {
  try {
    socket?.close();
  } catch {}
  chrome?.kill();
  server.kill();
  await wait(200);
}

for (const { what, ok, detail } of results) {
  console.log(`latex: ${ok ? "ok  " : "FAIL"}  ${what}${detail ? ` -- ${detail}` : ""}`);
}

if (MEASURE) {
  writeFileSync(join(CORPUS, "broken", "expected.json"), JSON.stringify(recorded, null, 2) + "\n");
  writeFileSync(join(CORPUS, "MEASUREMENTS.md"), table(measurements));
  console.log(`latex: measurements written to latex/corpus/MEASUREMENTS.md`);
}

if (failures) {
  console.error(`latex: ${failures} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`latex: ${results.length} checks passed in a real browser`);

function table(rows) {
  const mb = (n) => (n === null || n === undefined ? "--" : `${(n / 1e6).toFixed(2)} MB`);
  const s = (n) => (n === null || n === undefined ? "--" : `${n.toFixed(1)} s`);
  let out =
    "# The distributions, measured\n\n" +
    "Written by `node web/checks/latex.mjs --measure` against the local\n" +
    "mirror `node latex/tools/mirror.mjs` builds, in headless Chromium, on one machine.\n" +
    "Bytes are what the mirror actually served, counted by `latex/tools/serve.mjs`, with\n" +
    "Cache Storage emptied before each distribution and the byte tally reset before\n" +
    "each compile -- so a document's row is what a cold cache costs for that\n" +
    "document alone, over and above the up-front row.\n\n" +
    "| distribution | what | bytes fetched | cold | warm | pages |\n" +
    "| --- | --- | --- | --- | --- | --- |\n";
  for (const row of rows) {
    out += `| ${row.name} | ${row.what} | ${mb(row.bytes)} | ${s(row.cold)} | ${s(row.warm)} | ${row.pages ?? "--"} |\n`;
  }
  out +=
    "\nThe page counts a TeX Live on a desk gives the same documents are in\n" +
    "`latex/corpus/pages.json`, written by `node latex/tools/texlive.mjs`.\n" +
    "\nTwo things the table will mislead about if read quickly.\n\n" +
    "The **up front** row is what `choose` fetched before it resolved, and\n" +
    "that is not the same as what a distribution costs before its first page.\n" +
    "Both distributions defer most of their weight: SwiftLaTeX fetches a\n" +
    "module and then the LaTeX format and every package from inside the first\n" +
    "compile, and BusyTeX's pipeline constructs itself and then pulls its TeX\n" +
    "Live down when a document arrives. So the honest number for the card is\n" +
    "the up-front row **plus** the first document's row, and the first\n" +
    "document's row for a second document is the one under `paper`.\n\n" +
    "The **article** row is therefore the first-compile cost and the `paper`\n" +
    "row the steady-state one. That `paper` costs a quarter of a megabyte on\n" +
    "SwiftLaTeX and nothing at all on BusyTeX is the whole of the trade\n" +
    "between them, in two numbers.\n";
  return out;
}
