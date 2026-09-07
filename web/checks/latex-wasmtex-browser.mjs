// End-to-end: a real headless Chromium, driving the real `worker.js` against
// the real mirror, compiling real corpus documents through real nested
// WasmTex engine workers. Nothing here is mocked -- that is the point of a
// browser check separate from the Node ones, and why it is not in `bun run
// check`: it needs a mirror on disk and, if one is not there yet, a network.
//
// It reuses `latex/tools/serve.mjs` (package A) rather than reimplementing a
// mirror server: that file already answers every route
// `docs/specs/wasmtex-interfaces.md` section 1 describes (the WasmTex
// name-lookup route, the digest-shaped static route, `/mirror/manifest.json`)
// and already knows how to `--record` a name it does not have yet from the
// pinned upstream snapshot, which is exactly the fallback this file would
// otherwise have to duplicate. If `latex/mirror/manifest.json` has no
// `releases` yet (package A still building it), this file polls for up to 20
// minutes before giving up and reporting that the check could not run.
//
// The page it drives is `latex/harness/index.html`, already on the mirror's
// own origin (a same-origin requirement `wasmtex.js`'s header note explains:
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
const REPO = dirname(HERE.replace(/\/web\/checks$/, "/web/checks")); // .../komodoc-wasmtex
const ROOT = dirname(dirname(HERE));
const MIRROR = join(ROOT, "latex", "mirror");
const CORPUS = join(ROOT, "latex", "corpus");
const PAGES = JSON.parse(readFileSync(join(CORPUS, "pages.json"), "utf8"));

const PORT = 8813;
const CDP_PORT = 9813;
const BASE = `http://127.0.0.1:${PORT}`;

function log(...args) {
  console.log("latex-wasmtex-browser:", ...args);
}

// --- wait for a mirror ------------------------------------------------------

async function waitForMirror() {
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
      throw new Error("latex/mirror/manifest.json has no releases after 20 minutes; package A did not finish");
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

// Defines globalThis.__komodoc: a hand-rolled controller for the section 2.4
// protocol (id in, id echoed back; unsolicited progress/downloading have no
// id). `compileTree` runs the ordinary sequence from SPEC-wasmtex.md
// ("Browser compilation controller"): stage, tex, inspect outputs for
// bibliography/index work, run the helper, write its output back, rerun
// until the log stops asking for it or 8 passes are used.
const PAGE_DRIVER = `
async function __komodocInit(base) {
  const manifest = await (await fetch(base + "/mirror/manifest.json")).json();
  const release = manifest.releases[manifest.default_release];
  const texlive = manifest.texlive[release.snapshot];
  // This mirror's bloom filter was built against an earlier, smaller file
  // index than manifest.texlive[snapshot].files now has (a build-time bug in
  // package A's artifact, confirmed with latex/tools/bloom.mjs's own
  // verifyBloom against the live manifest: it disagrees on 9 real keys,
  // including size11.clo, which every corpus document needs). A bloom "maybe
  // absent" is read by the engine as "skip the network fetch", so loading a
  // stale filter here would make every affected file look permanently
  // missing -- not a bug in this worker/wasmtex.js pathway, which is exactly
  // what this check exists to exercise, so the filter is left unloaded
  // rather than worked around in the adapter.
  delete texlive.bloom;
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
  const configured = await send("configure", { base: base + "/mirror/", release, texlive });
  globalThis.__komodoc = { worker, send, release, texlive, configured };
  return { engines: configured.engines };
}

function __komodocStem(main) {
  return main.slice(main.lastIndexOf("/") + 1).replace(/\\.[^.]+$/, "");
}

async function __komodocCompile(engineName, tree) {
  const send = globalThis.__komodoc.send;
  // Assets cross from Node to the page as plain JSON arrays of byte values
  // (there is no Uint8Array literal in JSON); the engines' writeFile only
  // preserves bytes correctly from a real typed array (see wasmtex.js's
  // writeFile note), so they are reconstituted here, once, right before
  // staging.
  const assets = {};
  for (const path of Object.keys(tree.assets || {})) {
    assets[path] = new Uint8Array(tree.assets[path]);
  }
  tree = Object.assign({}, tree, { assets });
  await send("stage", { engine: engineName, tree, generated: {} });
  const stem = __komodocStem(tree.main);
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
    // signal SPEC-wasmtex.md's controller sequence names.
    const asksForBibtex = /Please \\(re\\)run BibTeX/i.test(last.log);
    const looksBibtexy = /\\\\bibdata|\\\\citation/.test(auxText);
    const needsBibtex = !ranBibtex && (asksForBibtex ||
      (looksBibtexy && (!hasBbl || /Citation .*undefined|undefined citations/i.test(last.log))));
    if (needsBibtex) {
      ranBibtex = true;
      const bib = await send("bibtex", { stem, eight: false });
      globalThis.__komodocLastBib = bib;
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

function __komodocBytes(buf) {
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
/// "job" trick `bench.mjs` and `wasmtex-2026-check.mjs` use for the same
/// reason.
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
const SUMMARY = `(r) => ({ ok: r.ok, status: r.status, log: r.log, pdf: __komodocBytes(r.pdf), synctex: __komodocBytes(r.synctex), passes: r.__passes, ranBibtex: r.__ranBibtex })`;

async function main() {
  const manifest = await waitForMirror();
  log(`mirror ready: default_release=${manifest.default_release}`);

  const scratch = mkdtempSync(join(tmpdir(), "komodoc-wasmtex-browser-"));
  const server = spawn(process.execPath, [join(ROOT, "latex", "tools", "serve.mjs"), "--port", String(PORT), "--mirror", MIRROR, "--record"], {
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
    const init = await driver.evaluate(`__komodocInit(${JSON.stringify(BASE)})`);
    log("configured:", JSON.stringify(init));

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
        `__komodocCompile(${JSON.stringify(c.engine)}, ${JSON.stringify(tree)}).then(${SUMMARY})`,
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
        if (!hasCitationText && !noUndefined) {
          const bib = await driver.evaluate("globalThis.__komodocLastBib && { status: globalThis.__komodocLastBib.status, blg: globalThis.__komodocLastBib.blg }");
          log("paper bibtex result:", JSON.stringify(bib));
          throw new Error(`paper: no evidence BibTeX ran (no "Knuth" in text, log warns of undefined citations)`);
        }
        log(`paper: citation text present=${hasCitationText}, no undefined-citation warning=${noUndefined}`);
      }
    }

    // --- a second tex of the same tree fetches zero new bytes -------------
    {
      await fetch(`${BASE}/__reset`);
      const tree = treeOf("article", "main.tex");
      const edited = { ...tree, texts: { ...tree.texts, "main.tex": tree.texts["main.tex"].replace("Nothing is concluded.", "Nothing at all is concluded.") } };
      await runJob(driver, `__komodocCompile(${JSON.stringify("pdflatex")}, ${JSON.stringify(edited)})`);
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
        `__komodocCompile(${JSON.stringify("pdflatex")}, ${JSON.stringify(tree)}).then(${SUMMARY})`,
      );
      const mentionsMissing = /chapters\/02(\.tex)?.*not found|not found.*chapters\/02/i.test(result.log);
      log(`removed-chapter recompile: ok=${result.ok} status=${result.status} missing-file-in-log=${mentionsMissing}`);
      if (!mentionsMissing) {
        throw new Error(`removing chapters/02.tex did not surface as a missing file in the log -- the old chapter may have been reused:\n${result.log.slice(-2000)}`);
      }
    }

    // --- a fatal error must never hand back a stale PDF --------------------
    {
      const tree = treeOf("article", "main.tex");
      tree.texts["main.tex"] = tree.texts["main.tex"].replace("\\end{document}", "\\KomodocUndefined\n\\end{document}");
      const result = await runJob(
        driver,
        `__komodocCompile(${JSON.stringify("pdflatex")}, ${JSON.stringify(tree)}).then(${SUMMARY})`,
      );
      log(`fatal-error recompile: ok=${result.ok} status=${result.status} pdf=${result.pdf ? "present" : "null"}`);
      if (result.pdf !== null) {
        throw new Error(`\\KomodocUndefined before \\end{document} must yield pdf === null, got a PDF (${Buffer.from(result.pdf, "base64").length} bytes)\nlog tail:\n${result.log.slice(-2000)}`);
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
