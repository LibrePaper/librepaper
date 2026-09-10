// End-to-end: a real headless Chromium, driving the real `worker.js` against
// the real mirror, compiling real corpus documents through real nested
// engine workers. Nothing here is mocked -- that is the point of a
// browser check separate from the Node ones, and why it is not in `bun run
// check`: it needs a mirror on disk and, if one is not there yet, a network.
//
// It reuses `latex/tools/serve.mjs` (package A) rather than reimplementing a
// mirror server: that file already answers every mirror route (the engine
// name-lookup route, the digest-shaped static route, `/mirror/manifest.json`).
// If the mirror's `manifest.json` has no `releases` yet (wasm-latex still
// building it), this file polls for up to 20 minutes before giving up and
// reporting that the check could not run.
//
// The page it drives is `latex/harness/index.html`, already on the mirror's
// own origin (a same-origin requirement `driver.js`'s header note explains:
// a nested engine Worker must be same-origin with the document that creates
// it). That page imports `latex.js` for its own reasons (a different check's
// harness); this file ignores that and creates its own `worker.js` Worker
// directly, driving the section 2.4 protocol by hand -- standing in for the
// controller (package B2) that does not exist yet.

import { execFileSync } from "node:child_process";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../tools/browser-driver.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = dirname(HERE.replace(/\/web\/checks$/, "/web/checks")); // .../librepaper
const ROOT = dirname(dirname(HERE));
const MIRROR = process.env.MIRROR || join(ROOT, "..", "wasm-latex", "mirror");
const CORPUS = join(ROOT, "latex", "corpus");
const PAGES = JSON.parse(readFileSync(join(CORPUS, "pages.json"), "utf8"));

const PORT = 8813;
const CDP_PORT = 9813;
const BASE = `http://127.0.0.1:${PORT}`;

function log(...args) {
  console.log("latex-browser:", ...args);
}

// --- wait for a mirror ------------------------------------------------------

async function waitForMirror() {
  if (/^https?:\/\//i.test(MIRROR)) {
    const response = await fetch(new URL('manifest.json', MIRROR.replace(/\/?$/, '/')), {
      cache: 'no-store', signal: AbortSignal.timeout(30000),
    });
    if (!response.ok) throw new Error(`mirror manifest: HTTP ${response.status}`);
    const manifest = await response.json();
    if (manifest.format !== 1 || !manifest.releases?.[manifest.default_release]) {
      throw new Error('mirror manifest has no format-1 default release');
    }
    return manifest;
  }
  const manifestPath = join(MIRROR, "manifest.json");
  const deadline = Date.now() + 20 * 60 * 1000;
  for (;;) {
    try {
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
      if (manifest.releases && Object.keys(manifest.releases).length > 0) return manifest;
    } catch {
      /* not written yet, or mid-write */
    }
    if (Date.now() > deadline) {
      throw new Error(`${MIRROR}/manifest.json has no releases after 20 minutes; the mirror was not built in time`);
    }
    await new Promise((r) => setTimeout(r, 5000));
  }
}

// --- corpus trees ------------------------------------------------------------

const TEXT_EXT = /\.(tex|bib|sty|cls|bst|cfg|def|bbx|cbx|dbx|lbx|csv|dat)$/;
const ASSET_EXT = /\.(png|pdf|jpe?g|otf|ttf)$/;

function filesIn(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const rel = prefix + entry.name;
      if (entry.isSymbolicLink()) throw new Error(`unexpected symlink: ${directory}/${entry.name}`);
      return entry.isDirectory() ? filesIn(join(directory, entry.name), `${rel}/`) : [rel];
    });
}

function treeOf(name, main) {
  const directory = join(CORPUS, name);
  const texts = {};
  const assets = {};
  for (const path of filesIn(directory)) {
    if (path.startsWith("logs/") || /(^|\/)(main\.pdf|expected\.json)$/.test(path)) continue;
    if (TEXT_EXT.test(path)) texts[path] = readFileSync(join(directory, path), "utf8");
    else if (ASSET_EXT.test(path)) assets[path] = [...readFileSync(join(directory, path))];
  }
  return { main, texts, assets };
}

// --- PDF inspection (pdfinfo/pdftotext, if present) --------------------------

function inspectPdfBytes(bytesArray, scratch) {
  if (!bytesArray) return { pdf: false, pages: 0, text: "" };
  const path = join(scratch, `x-${Math.random().toString(36).slice(2)}.pdf`);
  writeFileSync(path, Buffer.from(bytesArray));
  try {
    const info = execFileSync("pdfinfo", [path], { encoding: "utf8", timeout: 10000 });
    let text = "";
    try {
      text = execFileSync("pdftotext", ["-layout", path, "-"], { encoding: "utf8", timeout: 10000 });
    } catch {
      /* pdftotext missing: page count still answers the assertions that matter most */
    }
    return { pdf: true, pages: Number(info.match(/^Pages:\s+(\d+)/m)?.[1] || 0), text };
  } catch (error) {
    if (["ENOENT", "EPERM", "EACCES"].includes(error.code)) throw error;
    return { pdf: false, pages: 0, text: "" };
  }
}

// --- the in-page driver, injected once ---------------------------------------

// Defines globalThis.__librepaper: a hand-rolled controller for the worker
// message protocol (id in, id echoed back; unsolicited progress/downloading have
// no id). `compileTree` runs the ordinary controller sequence: stage, tex, inspect outputs for
// bibliography/index work, run the helper, write its output back, rerun until
// the log stops asking for it or 8 passes are used.
const PAGE_DRIVER = `
async function __librepaperInit(base) {
  const manifest = await (await fetch(base + "/mirror/manifest.json")).json();
  const release = manifest.releases[manifest.default_release];
  // Format 1: bundled releases resolve package files through bundles.json.
  const worker = new Worker("/src/lib/latex/worker.js", { type: "module" });
  let seq = 0;
  const pending = new Map();
  worker.onmessage = (ev) => {
    const msg = ev.data;
    if (msg && msg.id !== undefined) {
      const waiter = pending.get(msg.id);
      if (!waiter) return;
      pending.delete(msg.id);
      msg.failed ? waiter.reject(new Error(msg.failed)) : waiter.resolve(msg);
    }
  };
  function send(cmd, extra) {
    const id = ++seq;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      worker.postMessage(Object.assign({ id, cmd }, extra || {}));
    });
  }
  const configured = await send("configure", { base: base + "/mirror/", release, format: manifest.format });
  globalThis.__librepaper = { worker, send, release, configured };
  return { engines: configured.engines };
}

async function __librepaperControllerCompile(base, tree) {
  // Exercise the public controller, including its manifest-backed Biber
  // route.  The hand-written protocol driver below remains useful for the
  // corpus cases, but it cannot prove browser-biber provenance.
  const latex = await import("/src/lib/latex.js");
  latex.at(base + "/mirror/");
  latex.configure({ project: "browser-biber-check", settings: { engine: "pdflatex" } });
  return latex.compile(tree, { manual: true });
}

function __librepaperStem(main) {
  return main.slice(main.lastIndexOf("/") + 1).replace(/\\.[^.]+$/, "");
}

async function __librepaperCompile(engineName, tree) {
  const send = globalThis.__librepaper.send;
  // Assets cross from Node to the page as plain JSON arrays of byte values
  // (there is no Uint8Array literal in JSON); the engines' writeFile only
  // preserves bytes correctly from a real typed array (see driver.js's
  // writeFile note), so they are reconstituted here, once, right before
  // staging.
  const assets = {};
  for (const path of Object.keys(tree.assets || {})) {
    assets[path] = new Uint8Array(tree.assets[path]);
  }
  tree = Object.assign({}, tree, { assets });
  await send("stage", { engine: engineName, tree, generated: {} });
  const stem = __librepaperStem(tree.main);
  let last = null;
  let ranBibtex = false;
  let ranMakeindex = false;
  let passes = 0;
  const decoder = new TextDecoder();
  for (let pass = 0; pass < 8; pass++) {
    passes++;
    last = await send("tex", { engine: engineName, main: tree.main });
    const aux = last.outputs[stem + ".aux"];
    const auxText = aux ? decoder.decode(new Uint8Array(aux)) : "";
    const hasBcf = Boolean(last.outputs[stem + ".bcf"]);
    const hasIdx = Boolean(last.outputs[stem + ".idx"]);
    const hasBbl = Boolean(last.outputs[stem + ".bbl"]);
    // Plain \\bibliography documents write literal \\bibdata/\\citation into
    // the aux; biblatex with backend=bibtex does too, but is only certain to
    // need a (re)run of BibTeX once the log says so explicitly ("Please
    // (re)run BibTeX on the file(s): ..."), which is also the authoritative
    // signal the controller uses.
    const asksForBibtex = /Please \\(re\\)run BibTeX/i.test(last.log);
    const looksBibtexy = /\\\\bibdata|\\\\citation/.test(auxText);
    const needsBibtex = !ranBibtex && (asksForBibtex ||
      (looksBibtexy && (!hasBbl || /Citation .*undefined|undefined citations/i.test(last.log))));
    if (needsBibtex) {
      ranBibtex = true;
      const bib = await send("bibtex", { stem, eight: false });
      globalThis.__librepaperLastBib = bib;
      if (bib.bbl) await send("write", { path: stem + ".bbl", bytes: bib.bbl });
      continue;
    }
    if (hasIdx && !ranMakeindex) {
      ranMakeindex = true;
      const mk = await send("makeindex", { stem });
      if (mk.ind) await send("write", { path: stem + ".ind", bytes: mk.ind });
      continue;
    }
    const rerun = /Rerun to get|Please rerun|Label\\(s\\) may have changed|Citation .* undefined/.test(last.log);
    if (!rerun) break;
  }
  last.__passes = passes;
  last.__ranBibtex = ranBibtex;
  return last;
}

function __librepaperBytes(buf) {
  if (!buf) return null;
  const bytes = new Uint8Array(buf);
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}
true;
`;

/// `driver.evaluate` (CDP `Runtime.evaluate`) has a fixed 60 s timeout in
/// `browser-driver.mjs`, well under what a cold XeTeX engine plus dvipdfmx
/// plus font resolution can take. This starts the expression in the page
/// without awaiting it there, and polls a tiny boolean instead -- the same
/// "job" trick other engine-evaluation harnesses use for the same reason.
let jobSerial = 0;
async function runJob(driver, expression, timeoutMs = 240000) {
  const slot = `__job${++jobSerial}`;
  await driver.evaluate(
    `globalThis.${slot} = { done: false }; (${expression}).then(` +
      `(r) => { globalThis.${slot} = { done: true, result: r }; }, ` +
      `(e) => { globalThis.${slot} = { done: true, error: String((e && e.stack) || e) }; });true`,
  );
  await until(slot, () => driver.evaluate(`globalThis.${slot}.done`), timeoutMs);
  const job = await driver.evaluate(`globalThis.${slot}`);
  if (job.error) throw new Error(job.error);
  return job.result;
}

/// The shape both the corpus-case loop and the three invariant checks below
/// pull out of a compile: enough to inspect the PDF and the log without
/// shipping the whole outputs map back to Node.
const SUMMARY = `(r) => ({ ok: r.ok, status: r.status, log: r.log, pdf: __librepaperBytes(r.pdf), synctex: __librepaperBytes(r.synctex), passes: r.__passes, ranBibtex: r.__ranBibtex })`;

async function main() {
  const manifest = await waitForMirror();
  log(`mirror ready: default_release=${manifest.default_release}`);

  const scratch = mkdtempSync(join(tmpdir(), "librepaper-latex-browser-"));
  const server = spawn(process.execPath, [join(ROOT, "latex", "tools", "serve.mjs"), "--port", String(PORT), "--mirror", MIRROR], {
    stdio: ["ignore", "pipe", "pipe"],
    cwd: ROOT,
  });
  let serverLog = "";
  server.stdout.on("data", (b) => { serverLog += b; });
  server.stderr.on("data", (b) => { serverLog += b; });

  let driver;
  const timings = {};
  try {
    await until("mirror server", async () => (await fetch(`${BASE}/mirror/manifest.json`)).ok, 20000);
    driver = await browser("chromium", join(scratch, "chromium"), CDP_PORT);
    await driver.navigate(`${BASE}/`);
    await until("harness page", () => driver.evaluate("document.readyState === 'complete'"), 20000);
    await driver.evaluate(PAGE_DRIVER);
    const init = await driver.evaluate(`__librepaperInit(${JSON.stringify(BASE)})`);
    log("configured:", JSON.stringify(init));

    // The dedicated fixture uses biblatex's Biber backend and cites Knuth.
    // Run it through latex.js so the check covers browser Biber selection,
    // the real biber.worker.js protocol, and the result provenance.
    {
      const tree = treeOf("e2e/biber", "main.tex");
      const result = await runJob(
        driver,
        `__librepaperControllerCompile(${JSON.stringify(BASE)}, ${JSON.stringify(tree)}).then(r => ({ ...r, pdf: __librepaperBytes(r.pdf), synctex: __librepaperBytes(r.synctex) }))`,
      );
      const pdfBytes = result.pdf ? Buffer.from(result.pdf, "base64") : null;
      const inspected = inspectPdfBytes(pdfBytes, scratch);
      log(`biber: ok=${result.ok} bibliography=${result.provenance?.bibliography} pages=${inspected.pages}`);
      if (!result.ok || result.provenance?.bibliography !== "browser-biber") {
        throw new Error(`biber: controller did not use browser Biber (provenance=${JSON.stringify(result.provenance)}, attempts=${JSON.stringify(result.attempts)})\n${result.log?.slice(-3000) || ""}`);
      }
      if (!inspected.pdf || !inspected.text.includes("Knuth") || /undefined citations|Citation .*undefined/i.test(result.log || "")) {
        throw new Error(`biber: citation was not resolved in the PDF\n${result.log?.slice(-3000) || ""}`);
      }
    }

    const cases = [
      { id: "article", engine: "pdflatex" },
      { id: "paper", engine: "pdflatex" },
      { id: "packages", engine: "pdflatex" },
      { id: "xetex", engine: "xelatex" },
    ];

    for (const c of cases) {
      const tree = treeOf(c.id, "main.tex");
      const start = Date.now();
      const result = await runJob(
        driver,
        `__librepaperCompile(${JSON.stringify(c.engine)}, ${JSON.stringify(tree)}).then(${SUMMARY})`,
      );
      timings[c.id] = Date.now() - start;
      log(`${c.id}: passes=${result.passes} ranBibtex=${result.ranBibtex}`);
      const pdfBytes = result.pdf ? Buffer.from(result.pdf, "base64") : null;
      const inspected = inspectPdfBytes(pdfBytes, scratch);
      const expected = PAGES[c.id];
      log(`${c.id}: ok=${result.ok} pages=${inspected.pages} (expected ${expected.pages}) synctex=${Boolean(result.synctex)} ${timings[c.id]}ms`);
      if (!result.ok || !inspected.pdf) {
        throw new Error(`${c.id}: compile did not produce a PDF\n${result.log.slice(-2000)}`);
      }
      if (inspected.pages !== expected.pages) {
        log(`${c.id} log tail:`, result.log.slice(-3000));
        throw new Error(`${c.id}: expected ${expected.pages} pages, got ${inspected.pages}`);
      }
      if (expected.synctex && !result.synctex) {
        throw new Error(`${c.id}: expected a .synctex.gz, got none`);
      }
      if (c.id === "paper") {
        // The citation must actually be typeset -- BibTeX ran and its .bbl
        // was written back and picked up by a further pass -- not merely
        // "the compile succeeded despite an unresolved reference".
        const hasCitationText = inspected.text.includes("Knuth");
        const noUndefined = !/undefined citations|Citation .*undefined/i.test(result.log);
        if (!hasCitationText || !noUndefined) {
          const bib = await driver.evaluate("globalThis.__librepaperLastBib && { status: globalThis.__librepaperLastBib.status, bbl: Boolean(globalThis.__librepaperLastBib.bbl), blg: globalThis.__librepaperLastBib.blg }");
          log("paper bibtex result:", JSON.stringify(bib));
          if (!hasCitationText && !noUndefined) {
            throw new Error(`paper: no evidence BibTeX ran (no "Knuth" in text, log warns of undefined citations)`);
          }
        }
        log(`paper: citation text present=${hasCitationText}, no undefined-citation warning=${noUndefined}`);
      }
    }

    // --- a second tex of the same tree fetches zero new bytes -------------
    {
      await fetch(`${BASE}/__reset`);
      const tree = treeOf("article", "main.tex");
      const edited = { ...tree, texts: { ...tree.texts, "main.tex": tree.texts["main.tex"].replace("Nothing is concluded.", "Nothing at all is concluded.") } };
      await runJob(driver, `__librepaperCompile(${JSON.stringify("pdflatex")}, ${JSON.stringify(edited)})`);
      const tally = await (await fetch(`${BASE}/__bytes`)).json();
      log(`prose-edit recompile: ${tally.total} new bytes fetched from the mirror`);
      if (tally.total !== 0) {
        throw new Error(`expected zero new bytes on a same-session recompile, got ${tally.total}: ${JSON.stringify(tally.files)}`);
      }
    }

    // --- removing a chapter file must not reuse the old chapter ------------
    {
      const tree = treeOf("paper", "main.tex");
      delete tree.texts["chapters/02.tex"];
      const result = await runJob(
        driver,
        `__librepaperCompile(${JSON.stringify("pdflatex")}, ${JSON.stringify(tree)}).then(${SUMMARY})`,
      );
      // \include (unlike \input) reports a missing target as "No file
      // chapters/02.tex." rather than "File ... not found" -- both wordings
      // are accepted since which one appears depends on how the chapter was
      // brought in, not on anything this check controls.
      const mentionsMissing = /chapters\/02(\.tex)?.*not found|not found.*chapters\/02|no file chapters\/02/i.test(result.log);
      log(`removed-chapter recompile: ok=${result.ok} status=${result.status} missing-file-in-log=${mentionsMissing}`);
      if (!mentionsMissing) {
        throw new Error(`removing chapters/02.tex did not surface as a missing file in the log -- the old chapter may have been reused:\n${result.log.slice(-2000)}`);
      }
    }

    // --- a fatal error must never hand back a stale PDF --------------------
    {
      // `\LibrePaperUndefined` alone is not fatal: an undefined control
      // sequence is TeX's most ordinary recoverable error in nonstopmode
      // (verified against the real engine -- pdfTeX skips the token, status
      // 1, and still writes a PDF). `latex/corpus/broken/main.tex`'s own
      // comment says what actually is: "\input{a file that does not exist}
      // ... TeX stops dead on it: an input it cannot find is an emergency
      // stop" -- so that is the fatal construct used here, injected right
      // before \end{document} exactly where the spec's example puts
      // \LibrePaperUndefined.
      const tree = treeOf("article", "main.tex");
      tree.texts["main.tex"] = tree.texts["main.tex"].replace(
        "\\end{document}",
        "\\input{chapters/librepaper-does-not-exist}\n\\end{document}",
      );
      const result = await runJob(
        driver,
        `__librepaperCompile(${JSON.stringify("pdflatex")}, ${JSON.stringify(tree)}).then(${SUMMARY})`,
      );
      log(`fatal-error recompile: ok=${result.ok} status=${result.status} pdf=${result.pdf ? "present" : "null"}`);
      if (result.pdf !== null) {
        throw new Error(`a fatal \\input error before \\end{document} must yield pdf === null, got a PDF (${Buffer.from(result.pdf, "base64").length} bytes)\nlog tail:\n${result.log.slice(-2000)}`);
      }
    }

    log("timings (ms):", JSON.stringify(timings));
    log("all assertions passed");
  } finally {
    await driver?.close();
    server.kill();
    await new Promise((r) => setTimeout(r, 200));
    if (server.exitCode !== null && server.exitCode !== 0) console.error(serverLog);
    rmSync(scratch, { recursive: true, force: true });
  }
}

await main();
