#!/usr/bin/env node
// Populates the TeX Live half of the mirror by actually compiling the corpus
// through the real WasmTex engines in headless Chromium, against our own
// mirror server instead of upstream -- the same "compile it and see what it
// asks for" method `latex/tools/mirror.mjs --packages` documents for
// SwiftLaTeX, done here for WasmTex's own protocol.
//
// This is the one place in `latex/tools/` that imports WasmTex's `lib/
// engine/*.js` (the host-side driver classes, not the low-level worker
// controllers): nowhere else needs them, and they never reach `web/` -- the
// browser adapter that will eventually replace this recording harness is
// package B1's job (`web/src/lib/latex/wasmtex.js`, per
// `docs/specs/wasmtex-interfaces.md` section 2.4), written against the
// worker controllers directly, not against this SDK layer.
//
// One server, one origin: `serve.mjs --record --lib <checkout>/lib` is
// spawned as a child process. It is the actual mirror -- every engine and
// package fetch a compile makes goes through it, which is what populates
// `latex/mirror` -- and, with `--lib`, it also answers for `lib/engine/*.js`
// read live from the source checkout, so the harness page can `import` the
// driver classes. Both have to be the same origin: a Worker's script must be
// same-origin as the page that creates it, and the harness creates the
// engine workers by loading them straight from the mirror.
//
//     node latex/tools/wasmtex-record.mjs
//
// Requires the source checkout at latex/benchmark/candidates/wasmtex/source
// (see wasmtex.mjs's SOURCE_CHECKOUT) and a `chromium` on PATH.

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { browser, until } from "../../web/tools/browser-driver.mjs";
import { readManifest, writeManifest } from "./mirror.mjs";
import { ENGINE_RELEASE, SNAPSHOT } from "./wasmtex.mjs";

const HERE = dirname(new URL(import.meta.url).pathname);
const REPO = dirname(dirname(HERE));
const MIRROR = join(REPO, "latex", "mirror");
const LIB = join(REPO, "latex", "benchmark", "candidates", "wasmtex", "source", "lib");

const MIRROR_PORT = 8710;
const CDP_PORT = 9712;

/* ---------------------------------------------------- what to compile */

// Walks a directory the way a project tree is built: text files as strings,
// binary assets as bytes, everything else (logs, expected.json, PDFs already
// committed as fixtures) left out. Deliberately not imported from
// `latex/benchmark/corpus.mjs` -- this file's only benchmark dependency is
// the pinned manifest JSON `wasmtex.mjs` already reads; a few lines of
// directory-walking are not worth reaching further into that tree for.
function treeOf(directory, main) {
  const texts = {};
  const assets = {};
  const walk = (dir, prefix) => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const path = prefix + entry.name;
      if (entry.isDirectory()) {
        if (path === "logs") continue;
        walk(join(dir, entry.name), `${path}/`);
        continue;
      }
      if (path === "expected.json" || path.endsWith(".pdf") && path !== main) continue;
      if (!/\.(tex|bib|sty|cls|bst|cfg|def|bbx|cbx|dbx|lbx|csv|dat|png|pdf|jpe?g|otf|ttf)$/.test(path)) continue;
      const bytes = readFileSync(join(dir, entry.name));
      if (/\.(png|pdf|jpe?g|otf|ttf)$/.test(path)) assets[path] = bytes;
      else texts[path] = bytes.toString("utf8");
    }
  };
  walk(directory, "");
  return { main, texts, assets };
}

const LUATEX_MINI = {
  id: "luatex-mini",
  engine: "luatex",
  bib: false,
  tree: {
    main: "main.tex",
    texts: {
      "main.tex":
        "% A tiny fontspec document. The corpus has no LuaTeX case (the thesis\n" +
        "% fixture `latex/benchmark/.cache/projects/thesis` was not present when this\n" +
        "% ran); this exists so LuaHBTeX and fontspec/luaotfload are recorded too.\n" +
        "\\documentclass{article}\n" +
        "\\usepackage{fontspec}\n" +
        "\\begin{document}\n" +
        "LuaTeX with fontspec.\n" +
        "\\end{document}\n",
    },
    assets: {},
  },
};

function documents() {
  const docs = [
    { id: "article", engine: "pdftex", bib: true, tree: treeOf(join(REPO, "latex", "corpus", "article"), "main.tex") },
    { id: "paper", engine: "pdftex", bib: true, tree: treeOf(join(REPO, "latex", "corpus", "paper"), "main.tex") },
    // biblatex/biber, not plain BibTeX: WasmTex has no full Biber backend
    // (docs/specs/wasmtex.md), so this is compiled for its package requests only --
    // undefined citations in the log are expected, not a recording failure.
    { id: "packages", engine: "pdftex", bib: false, tree: treeOf(join(REPO, "latex", "corpus", "packages"), "main.tex") },
    { id: "xetex", engine: "xetex", bib: false, tree: treeOf(join(REPO, "latex", "corpus", "xetex"), "main.tex") },
    { id: "unicode-fonts", engine: "xetex", bib: false, tree: treeOf(join(REPO, "latex", "benchmark", "fixtures", "unicode-fonts"), "main.tex") },
    LUATEX_MINI,
  ];
  // The wider corpus (acm-conference, biber-related, biber-sorting,
  // multifile, thesis) lives under a cache `prepare.mjs` fills by fetching
  // upstream document sources; it is not populated in this checkout (no
  // `latex/benchmark/.cache/projects`), so those cases are skipped here
  // rather than silently faked. See the tool's final report.
  const projects = join(REPO, "latex", "benchmark", ".cache", "projects");
  if (existsSync(projects)) {
    console.log(`wasmtex-record: NOTE: ${projects} exists but its cases are not wired into this tool yet`);
  }
  const only = process.env.WASMTEX_RECORD_ONLY?.split(",");
  return only ? docs.filter((d) => only.includes(d.id)) : docs;
}

/* --------------------------------------------------------------- server */

function spawnMirrorServer() {
  const child = spawn(
    process.execPath,
    [join(HERE, "serve.mjs"), "--port", String(MIRROR_PORT), "--mirror", MIRROR, "--record", "--lib", LIB],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  child.stdout.on("data", (chunk) => process.stdout.write(`serve: ${chunk}`));
  child.stderr.on("data", (chunk) => process.stderr.write(`serve: ${chunk}`));
  return child;
}

/* -------------------------------------------------------- browser-side */

// Runs inside the page (see `driver.evaluate` below): compiles one project
// tree with the given engine, TeX -> BibTeX -> TeX -> TeX when `bib` is set.
// Written as one self-contained function, exactly as
// `latex/benchmark/candidates/comparison/wasmtex-2026-check.mjs` does it,
// because it crosses into `Function.prototype.toString()` to reach the page.
async function compileInPage(doc, opts) {
  const { WasmTexPdftexEngine } = await import("/lib/engine/wasmtex-engine.js");
  const { WasmTexXetexEngine } = await import("/lib/engine/xetex-engine.js");
  const { WasmTexLuatexEngine } = await import("/lib/engine/luatex-engine.js");
  const { BibtexEngine } = await import("/lib/engine/bibtex-engine.js");
  const Engine = { pdftex: WasmTexPdftexEngine, xetex: WasmTexXetexEngine, luatex: WasmTexLuatexEngine }[doc.engine];
  const tex = new Engine(opts);
  const phases = [];
  try {
    await tex.init();
    // Parent directories first, or a nested writeFile fails: the worker's
    // FS.writeFile is not recursive (see wasm-build/pdftex-worker.js
    // writeFileRoutine).
    const dirs = new Set();
    for (const path of [...Object.keys(doc.tree.texts), ...Object.keys(doc.tree.assets)]) {
      const parts = path.split("/");
      for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
    }
    for (const dir of [...dirs].sort()) await tex.mkdir(dir);
    for (const [path, content] of Object.entries(doc.tree.texts)) await tex.writeFile(path, content);
    for (const [path, bytes] of Object.entries(doc.tree.assets)) await tex.writeFile(path, bytes);
    tex.setMainFile(doc.tree.main);

    const first = await tex.compile();
    phases.push({ stage: "tex1", success: first.success, log: first.log });

    if (doc.bib && first.success) {
      const stem = doc.tree.main.replace(/\.tex$/, "");
      const aux = await tex.readFile(`${stem}.aux`);
      // \bibdata{a,b,c} names every .bib TeX opened for this run, including
      // ones written only by \begin{filecontents*} during pass 1 (the
      // `article` case), which are not in the original project tree.
      // `readFile` defaults to utf8 decoding on the worker side (see
      // wasm-build/pdftex-worker.js's readfile handler), so `aux` is
      // already a string here, not bytes to decode.
      const auxText = typeof aux === "string" ? aux : new TextDecoder().decode(aux);
      const bibdata = /\\bibdata\{([^}]*)\}/.exec(auxText)?.[1]?.split(",") ?? [];
      const bib = new BibtexEngine(opts);
      await bib.init();
      await bib.writeFile(`${stem}.aux`, aux);
      for (const name of bibdata) {
        const bytes = await tex.readFile(`${name.trim()}.bib`);
        if (bytes) await bib.writeFile(`${name.trim()}.bib`, bytes);
      }
      const br = await bib.compile(stem);
      phases.push({ stage: "bibtex", success: br.success, log: br.log });
      const bbl = await bib.readFile(`${stem}.bbl`);
      bib.terminate();
      if (bbl) {
        await tex.writeFile(`${stem}.bbl`, bbl);
        const second = await tex.compile();
        phases.push({ stage: "tex2", success: second.success, log: second.log });
        const third = await tex.compile();
        phases.push({ stage: "tex3", success: third.success, log: third.log });
      }
    } else if (first.success) {
      // No bibliography step, but a document a compiler cares about is
      // stable after one rerun; a second pass is cheap and catches anything
      // the resolver only asks for once cross-references exist.
      const second = await tex.compile();
      phases.push({ stage: "tex2", success: second.success, log: second.log });
    }
    return { id: doc.id, engine: doc.engine, phases };
  } finally {
    tex.terminate();
  }
}

/* --------------------------------------------------------------------- run */

async function main() {
  if (!existsSync(LIB)) {
    throw new Error(
      `wasmtex-record: no source checkout lib/ at ${LIB}. See wasmtex.mjs's SOURCE_CHECKOUT and\n` +
        "  docs/specs/wasmtex.md \"Starting point\" for the revision to check out.",
    );
  }

  // A bloom filter built from a smaller, earlier set of recorded keys is
  // worse than no bloom filter at all here: the engine loads it once at
  // init and then trusts a negative absolutely (that is the whole point of
  // `bloomMaybe` -- see wasm-build/pdftex-worker.js), so a stale filter
  // silently skips the very fetches this run exists to discover. `files`
  // and `absent` already recorded stay -- they are real, verified fetches
  // -- only the filter itself is reset; `serve.mjs`'s shutdown handler
  // rebuilds it from whatever is recorded by the time this run ends.
  const manifestBefore = readManifest(MIRROR);
  const entry = manifestBefore.texlive?.[SNAPSHOT];
  if (entry?.bloom) {
    delete entry.bloom;
    writeManifest(manifestBefore, MIRROR);
  }
  const bloomPath = join(MIRROR, "texlive", SNAPSHOT, "bloom-filter.v2.bin");
  if (existsSync(bloomPath)) rmSync(bloomPath);

  const before = readManifest(MIRROR).texlive?.[SNAPSHOT]?.files ?? {};
  const beforeKeys = new Set(Object.keys(before));
  console.log(`wasmtex-record: starting with ${beforeKeys.size} package file(s) already recorded`);

  const mirror = spawnMirrorServer();
  await until("mirror server", async () => {
    try {
      await fetch(`http://127.0.0.1:${MIRROR_PORT}/mirror/manifest.json`);
      return true;
    } catch {
      return false;
    }
  });

  const profile = mkdtempSync(join(tmpdir(), "librepaper-wasmtex-record-"));
  let driver;
  const results = [];
  const initialSources = ["article", "paper", "packages"];
  const initialKeys = new Set();
  try {
    driver = await browser("chromium", profile, CDP_PORT);
    await driver.navigate(`http://127.0.0.1:${MIRROR_PORT}/`);
    await until("page", () => driver.evaluate('document.readyState === "complete"'), 10000);

    for (const doc of documents()) {
      const beforeThis = readManifest(MIRROR).texlive?.[SNAPSHOT]?.files ?? {};
      const opts = {
        assetBaseUrl: `http://127.0.0.1:${MIRROR_PORT}/mirror/`,
        texliveVersion: ENGINE_RELEASE,
        texliveUrl: `http://127.0.0.1:${MIRROR_PORT}/mirror/texlive/${SNAPSHOT}/`,
        disablePreambleSnapshot: true,
        persistentCache: false,
      };
      const call =
        `globalThis.job={};(${compileInPage.toString()})(${JSON.stringify(doc)},${JSON.stringify(opts)})` +
        `.then(r=>globalThis.job={done:true,result:r},e=>globalThis.job={done:true,error:String(e && e.stack || e)});true`;
      await driver.evaluate(call);
      await until(`compile ${doc.id}`, () => driver.evaluate("globalThis.job.done"), 180000);
      const job = await driver.evaluate("globalThis.job");
      if (job.error) {
        results.push({ id: doc.id, engine: doc.engine, error: job.error });
        console.log(`wasmtex-record: ${doc.id.padEnd(14)} FAILED: ${job.error}`);
      } else {
        results.push(job.result);
        const summary = job.result.phases.map((p) => `${p.stage}=${p.success ? "ok" : "FAIL"}`).join(" ");
        console.log(`wasmtex-record: ${doc.id.padEnd(14)} ${summary}`);
        if (process.env.WASMTEX_RECORD_DEBUG) {
          for (const p of job.result.phases) {
            if (!p.success) console.log(`wasmtex-record:   ${doc.id}/${p.stage} log tail:\n${p.log.slice(-1500)}`);
          }
        }
      }
      if (initialSources.includes(doc.id)) {
        const afterThis = readManifest(MIRROR).texlive?.[SNAPSHOT]?.files ?? {};
        for (const key of Object.keys(afterThis)) initialKeys.add(key);
        void beforeThis; // recorded for readability; the running union is what matters
      }
    }
  } finally {
    await driver?.close();
    rmSync(profile, { recursive: true, force: true });
    // SIGINT is `serve.mjs`'s own shutdown path: it writes the manifest and
    // regenerates the bloom filter there, see serve.mjs's signal handlers.
    mirror.kill("SIGINT");
    await new Promise((resolve) => mirror.once("exit", resolve));
  }

  const after = readManifest(MIRROR).texlive?.[SNAPSHOT]?.files ?? {};
  const added = Object.keys(after).filter((key) => !beforeKeys.has(key));
  const bytes = added.reduce((sum, key) => sum + (after[key]?.size || 0), 0);
  console.log(`wasmtex-record: recorded ${added.length} new package file(s), ${(bytes / 1e6).toFixed(1)} MB`);

  if (initialKeys.size) {
    const setInitial = spawn(
      process.execPath,
      [join(HERE, "wasmtex.mjs"), "--initial", ...initialKeys],
      { stdio: "inherit" },
    );
    await new Promise((resolve, reject) => {
      setInitial.once("exit", (code) => (code === 0 ? resolve() : reject(new Error(`--initial exited ${code}`))));
    });
  }

  const failed = results.filter((r) => r.error || r.phases?.some((p) => !p.success && p.stage === "tex1"));
  if (failed.length) {
    console.log(`wasmtex-record: ${failed.length} document(s) did not typeset cleanly (see above); mirror was still populated`);
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
