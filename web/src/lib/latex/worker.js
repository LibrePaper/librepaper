// The compile, off the main thread -- WasmTex edition.
//
// This is the module worker `latex/route.js` and the controller
// (`web/src/lib/latex.js`, package B2) talk to. It owns one release at a
// time and lazily creates the nested WasmTex engine workers it needs
// (`wasmtex.js`, one per pdfTeX/XeTeX/dvipdfm/LuaTeX/BibTeX/BibTeX8/
// makeindex), replacing the old two-message SwiftLaTeX protocol with the one
// in `docs/specs/wasmtex-interfaces.md` section 2.4:
//
//   in  { id, cmd: "configure", base, release, texlive }
//   in  { id, cmd: "stage", engine, tree, generated }
//   in  { id, cmd: "tex", engine, main }
//   in  { id, cmd: "bibtex", stem, eight }
//   in  { id, cmd: "makeindex", stem }
//   in  { id, cmd: "write", path, bytes }
//   in  { id, cmd: "read", path }
//   in  { id, cmd: "retire" }
//   out { id, ok: true, ... } | { id, failed: "message" }
//   out { cmd: "progress", done, total, scope }   -- unsolicited
//   out { cmd: "downloading", file }              -- unsolicited
//
// Every command carries an id and every reply echoes it, unlike the old
// protocol (`ready`/`compiled`/`failed` with no correlation at all), because
// this worker now fields several kinds of request -- a `bibtex` call can be
// in flight while a `read` for an unrelated file arrives -- and there is no
// longer exactly one request in the air at a time.
//
// The GLUE map from the old protocol is gone: there is exactly one adapter
// now (`wasmtex.js`), selected by engine kind, not by distribution name.

import { createEngine } from "./wasmtex.js";
import { prefetch, fetchVerified } from "./resources.js";

/// Which underlying engine kinds a project engine name needs, in the order
/// their controllers should be driven: the first is the one that runs LaTeX
/// itself, the rest (dvipdfm, for XeTeX) receive the whole project tree too
/// -- dvipdfmx resolves images and embedded fonts against project-relative
/// paths on its own, exactly as `xetex-engine.js`'s own `writeFile` override
/// mirrors every project write to both engines.
const KINDS = {
  pdflatex: ["pdftex"],
  xelatex: ["xetex", "dvipdfm"],
  lualatex: ["luatex"],
};

const GZIP_MAGIC = [0x1f, 0x8b];

async function ensureGzip(bytes) {
  if (!bytes || !bytes.length) return null;
  if (bytes[0] === GZIP_MAGIC[0] && bytes[1] === GZIP_MAGIC[1]) return bytes; // already gzip
  if (typeof CompressionStream === "undefined") return bytes; // best effort
  const stream = new Response(bytes).body.pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/// TeX names its outputs after the job name, not the path it was given: a
/// compile of `paper/main.tex` writes `main.pdf`, not `paper/main.pdf`.
function stemOf(path) {
  const base = path.slice(path.lastIndexOf("/") + 1);
  return base.replace(/\.[^.]+$/, "");
}

function dirname(path) {
  const at = path.lastIndexOf("/");
  return at < 0 ? null : path.slice(0, at);
}

const OUTPUT_EXTENSIONS = ["aux", "bcf", "idx", "ind", "ilg", "bbl", "blg", "toc", "lof", "lot", "out", "run.xml"];

class Worker2 {
  constructor() {
    this.base = null;
    this.release = null;
    this.texlive = null;
    this.engines = new Map(); // kind -> wasmtex engine
    this.initialFiles = []; // [{format, name, bytes}]
    this.absentEntries = []; // [{format, filename}]
    this.bloomBytes = null;
    // The most recently staged snapshot: bibtex/makeindex need the project's
    // .bib/.bst/.idx-adjacent files and the engine name to know which
    // primary engine holds the .aux they run against, and neither travels on
    // the bibtex/makeindex command itself (section 2.4).
    this.tree = null;
    this.engineName = null;
  }

  async handle(message) {
    const { id, cmd } = message;
    switch (cmd) {
      case "configure":
        return { id, ...(await this.configure(message)) };
      case "stage":
        return { id, ...(await this.stage(message)) };
      case "tex":
        return { id, ...(await this.tex(message)) };
      case "bibtex":
        return { id, ...(await this.bibtex(message)) };
      case "makeindex":
        return { id, ...(await this.makeindex(message)) };
      case "write":
        return { id, ...(await this.write(message)) };
      case "read":
        return { id, ...(await this.read(message)) };
      case "retire":
        return { id, ...(await this.retire()) };
      default:
        throw new Error(`unknown command ${cmd}`);
    }
  }

  // -- configure --------------------------------------------------------

  async configure({ base, release, texlive }) {
    if (this.release && this.release.id !== release?.id) {
      // A different release: nothing initialized against the old one is
      // safe to keep (different engine builds, different TeX Live).
      await this.retire();
    }
    this.base = base;
    this.release = release;
    this.texlive = texlive ?? null;
    this.initialFiles = [];
    this.absentEntries = [];
    this.bloomBytes = null;

    if (this.texlive?.bloom) {
      try {
        const response = await fetchVerified(this.release, resolve(this.base, this.texlive.bloom.url), {
          sha256: this.texlive.bloom.sha256,
          size: this.texlive.bloom.size,
        });
        this.bloomBytes = new Uint8Array(await response.arrayBuffer());
      } catch {
        // The bloom filter is a resolver optimisation, not a requirement:
        // its absence just means more individual lookups.
      }
    }

    if (this.texlive?.absent) {
      for (const key of Object.keys(this.texlive.absent)) {
        const parsed = parseTexliveKey(key);
        if (parsed) this.absentEntries.push({ format: parsed.format, filename: parsed.name });
      }
    }

    const initial = Array.isArray(this.texlive?.initial) ? this.texlive.initial : [];
    const entries = [];
    for (const key of initial) {
      const info = this.texlive?.files?.[key];
      const parsed = parseTexliveKey(key);
      if (!info || !parsed) continue;
      entries.push({ key, parsed, url: resolve(this.base, info.url), sha256: info.sha256, size: info.size, scope: "initial resources" });
    }
    if (entries.length) {
      const responses = await prefetch(this.release, entries, (progress) => this.emit("progress", progress));
      for (let i = 0; i < entries.length; i++) {
        const response = responses[i];
        if (!response) continue;
        const bytes = new Uint8Array(await response.arrayBuffer());
        this.initialFiles.push({ format: entries[i].parsed.format, name: entries[i].parsed.name, bytes });
      }
    }

    return { ok: true, engines: Object.keys(this.release?.engines ?? {}) };
  }

  emit(cmd, extra) {
    self.postMessage({ cmd, ...extra });
  }

  // -- engine lifecycle ---------------------------------------------------

  async ensureEngine(kind) {
    const existing = this.engines.get(kind);
    if (existing) return existing;
    const spec = this.release?.engines?.[kind];
    if (!spec) throw new Error(`release ${this.release?.id} has no ${kind} engine`);
    const workerUrl = resolve(this.base, `${this.release.base}${spec.worker}`);
    const texliveUrl = resolve(this.base, this.release.texlive_base);
    let format = null;
    if (spec.format) {
      const info = this.release.files?.[spec.format];
      if (info) {
        format = {
          url: resolve(this.base, info.url),
          sha256: info.sha256,
          size: info.size,
          gz: spec.format.endsWith(".gz"),
        };
      }
    }
    const engine = createEngine({
      kind,
      url: workerUrl,
      texliveUrl,
      format,
      release: this.release,
      onProgress: (progress) => this.emit("progress", progress),
      onDownload: (file) => this.emit("downloading", { file }),
    });
    this.engines.set(kind, engine);
    try {
      await engine.init();
    } catch (error) {
      this.engines.delete(kind);
      throw error;
    }
    // Inject the compact initial set and the bloom filter before this
    // engine's first pass -- never fetched per-engine beyond what
    // `configure` already prefetched once for the whole release.
    if (this.bloomBytes) engine.loadBloom(this.bloomBytes);
    if (this.absentEntries.length) await engine.preload404(this.absentEntries);
    for (const file of this.initialFiles) {
      await engine.preloadTexlive(file.format, file.name, file.bytes);
    }
    return engine;
  }

  // -- stage ----------------------------------------------------------------

  async stage({ engine: engineName, tree, generated }) {
    const kinds = KINDS[engineName];
    if (!kinds) throw new Error(`unknown engine ${engineName}`);
    this.tree = tree;
    this.engineName = engineName;
    const engines = [];
    for (const kind of kinds) {
      const engine = await this.ensureEngine(kind);
      await engine.flushCache();
      engines.push(engine);
    }
    // Fresh /work for every engine, then the whole tree, one directory at a
    // time -- `flushcache` above is the only way any of these controllers
    // clears `/work`, so a file removed from the tree is genuinely absent
    // from the next compile rather than merely overwritten.
    for (const engine of engines) {
      await writeTree(engine, tree);
    }
    // Generated aux/bbl/ind from an earlier pass of the same snapshot: only
    // the TeX engine reads these (dvipdfm has no use for an .aux), so they
    // go to the primary engine alone.
    if (generated && Object.keys(generated).length) {
      await writeFiles(engines[0], generated);
    }
    return { ok: true };
  }

  // -- tex --------------------------------------------------------------

  async tex({ engine: engineName, main }) {
    const kinds = KINDS[engineName];
    if (!kinds) throw new Error(`unknown engine ${engineName}`);
    const primary = this.engines.get(kinds[0]);
    if (!primary) throw new Error(`${engineName} was not staged`);
    const stem = stemOf(main);

    // No controller exposes a delete/unlink command -- `flushcache` is the
    // only way to clear a file, and calling it here would also drop the
    // format and every TeX Live file just preloaded. Instead, the output
    // file is overwritten with zero bytes before the pass; a run that does
    // not produce fresh output this time leaves it empty, and an empty
    // read-back is treated as "no PDF" below, which is what "delete before
    // each pass" is really protecting against: a stale PDF from a prior
    // successful pass being handed back for a run that produced nothing.
    await primary.writeFile(`${stem}.pdf`, new Uint8Array(0));
    if (engineName === "xelatex") await primary.writeFile(`${stem}.xdv`, new Uint8Array(0));
    primary.setMainFile(main);

    const pass = await primary.run("compilelatex");
    let pdf = pass.pdf && pass.pdf.length ? pass.pdf : null;
    let synctexRaw = pass.synctex && pass.synctex.length ? pass.synctex : null;
    let log = pass.log;
    let status = pass.status;
    let ok = pass.ok;

    if (engineName === "xelatex") {
      // XeTeX's own controller hands back the .xdv content in the same
      // `pdf` field a real PDF would occupy (see wasmtex.js's header note on
      // reply-shape quirks); it is not a PDF until dvipdfm has run.
      const xdv = pdf;
      pdf = null;
      const dvipdfm = this.engines.get("dvipdfm");
      if (pass.ok && xdv && dvipdfm) {
        await dvipdfm.writeFile(`${stem}.pdf`, new Uint8Array(0));
        await dvipdfm.writeFile(`${stem}.xdv`, xdv);
        dvipdfm.setMainFile(`${stem}.xdv`);
        const made = await dvipdfm.run("compilepdf");
        log = `${log}\n${made.log}`;
        pdf = made.ok && made.pdf && made.pdf.length ? made.pdf : null;
        ok = made.ok && !!pdf;
        status = made.status;
      } else {
        ok = false;
      }
    }
    if (!pdf) ok = false;

    // XeTeX's controller reply never carries a `synctex` field at all
    // (unlike pdfTeX's and LuaTeX's -- see wasmtex.js's header note), but
    // XeTeX still writes the file to /work when run with synctex enabled;
    // read it back directly rather than relying on the reply shape.
    if (!synctexRaw && ok) {
      synctexRaw =
        (await primary.readFile(`${stem}.synctex`, true)) ||
        (await primary.readFile(`${stem}.synctex.gz`, true)) ||
        null;
    }
    const synctex = ok ? await ensureGzip(synctexRaw) : null;
    const outputs = await collectOutputs(primary, stem, pass.inputs);

    const transfer = [];
    if (pdf) transfer.push(pdf.buffer);
    if (synctex) transfer.push(synctex.buffer);
    const outputBuffers = {};
    for (const [path, bytes] of Object.entries(outputs)) {
      outputBuffers[path] = bytes.buffer;
      transfer.push(bytes.buffer);
    }

    return {
      ok: true,
      status,
      pdf: pdf ? pdf.buffer : null,
      synctex: synctex ? synctex.buffer : null,
      log,
      inputs: pass.inputs,
      outputs: outputBuffers,
      __transfer: transfer,
    };
  }

  // -- bibtex / makeindex -------------------------------------------------

  async bibtex({ stem, eight }) {
    const kinds = KINDS[this.engineName];
    const primary = kinds && this.engines.get(kinds[0]);
    if (!primary) throw new Error("no staged engine to read the .aux from");
    const aux = await primary.readFile(`${stem}.aux`, true);
    if (!aux) throw new Error(`${stem}.aux is not staged`);
    const kind = eight ? "bibtex8" : "bibtex";
    const helper = await this.ensureEngine(kind);
    await helper.writeFile(`${stem}.aux`, aux);
    // BibTeX follows every `\@input{...}` in the main aux itself (a
    // multi-file document's \citation and \bibdata commands live in the
    // chapter aux files \include wrote, not in the top-level one) -- without
    // these staged too, BibTeX reports "I couldn't open auxiliary file" for
    // each and finds no citations at all.
    let auxText = new TextDecoder().decode(aux);
    for (const path of nestedAuxPaths(aux)) {
      const bytes = await primary.readFile(path, true);
      if (!bytes) continue;
      await ensureDirs(helper, path);
      await helper.writeFile(path, bytes);
      auxText += new TextDecoder().decode(bytes);
    }
    await stageProjectFiles(helper, this.tree, [".bib", ".bst"]);
    // A `\bibdata{...}` name is not always a project source file: `biblatex`
    // in backend=bibtex mode (and a plain `\begin{filecontents*}{x.bib}...`)
    // writes its .bib into the *primary engine's* own /work at compile time,
    // never into the tree this worker was staged from. Any such name that
    // `stageProjectFiles` did not already cover is read back from the
    // primary engine's filesystem instead.
    for (const name of bibdataNames(auxText)) {
      const path = `${name}.bib`;
      const alreadyStaged = pathIn(this.tree, path);
      if (alreadyStaged) continue;
      const bytes = await primary.readFile(path, true);
      if (!bytes) continue;
      await ensureDirs(helper, path);
      await helper.writeFile(path, bytes);
    }
    const pass = await helper.run(eight ? "compilebibtex8" : "compilebibtex", { url: stem });
    const bbl = await helper.readFile(`${stem}.bbl`, true);
    const blg = (await helper.readFile(`${stem}.blg`, false)) ?? "";
    const transfer = bbl ? [bbl.buffer] : [];
    return { ok: true, status: pass.ok ? 0 : 1, bbl: bbl ? bbl.buffer : null, blg, __transfer: transfer };
  }

  async makeindex({ stem }) {
    const kinds = KINDS[this.engineName];
    const primary = kinds && this.engines.get(kinds[0]);
    if (!primary) throw new Error("no staged engine to read the .idx from");
    const idx = await primary.readFile(`${stem}.idx`, true);
    if (!idx) throw new Error(`${stem}.idx is not staged`);
    const helper = await this.ensureEngine("makeindex");
    await helper.writeFile(`${stem}.idx`, idx);
    await stageProjectFiles(helper, this.tree, [".ist"]);
    const pass = await helper.run("compilemakeindex", { url: stem });
    const ind = await helper.readFile(`${stem}.ind`, true);
    const ilg = (await helper.readFile(`${stem}.ilg`, false)) ?? "";
    const transfer = ind ? [ind.buffer] : [];
    return { ok: true, status: pass.ok ? 0 : 1, ind: ind ? ind.buffer : null, ilg, __transfer: transfer };
  }

  // -- write / read -----------------------------------------------------

  async write({ path, bytes }) {
    const kinds = this.engineName ? KINDS[this.engineName] : [];
    for (const kind of kinds) {
      const engine = this.engines.get(kind);
      if (engine) await engine.writeFile(path, new Uint8Array(bytes));
    }
    return { ok: true };
  }

  async read({ path }) {
    const kinds = this.engineName ? KINDS[this.engineName] : [];
    const primary = kinds.length ? this.engines.get(kinds[0]) : null;
    if (!primary) return { ok: true, bytes: null };
    const bytes = await primary.readFile(path, true);
    return { ok: true, bytes: bytes ? bytes.buffer : null, __transfer: bytes ? [bytes.buffer] : [] };
  }

  // -- retire -------------------------------------------------------------

  async retire() {
    for (const engine of this.engines.values()) engine.terminate();
    this.engines.clear();
    this.tree = null;
    this.engineName = null;
    return { ok: true };
  }
}

function resolve(base, path) {
  return new URL(path, base ?? self.location.href).href;
}

/// `texlive.files`/`texlive.absent` keys are `<engine>/<kpathsea format
/// code>/<name>`; the engine segment is always the literal `pdftex` even for
/// XeTeX/LuaTeX file lookups (section 1), so only the format code and bare
/// name are meaningful here.
function parseTexliveKey(key) {
  const parts = key.split("/");
  if (parts.length < 3) return null;
  const format = Number(parts[1]);
  if (!Number.isFinite(format)) return null;
  return { format, name: parts.slice(2).join("/") };
}

async function ensureDirs(engine, path) {
  const dir = dirname(path);
  // A file at the root has no directory to make, and the engine's
  // `mkdir .` is an error it prints to the console for every file.
  if (dir && dir !== "." && dir !== "/") await engine.mkdir(dir);
}

async function writeFiles(engine, files) {
  for (const [path, contents] of Object.entries(files ?? {})) {
    await ensureDirs(engine, path);
    await engine.writeFile(path, contents);
  }
}

async function writeTree(engine, tree) {
  await writeFiles(engine, tree.texts ?? {});
  await writeFiles(engine, tree.assets ?? {});
}

/// The nested aux paths a `\@input{...}` line in the main aux points at
/// (from `\include`d chapters). `bytes` is the main aux's own content.
function nestedAuxPaths(bytes) {
  const text = new TextDecoder().decode(bytes);
  return [...text.matchAll(/\\@input\{([^}]+)\}/g)].map((match) => match[1]);
}

/// `\bibdata{name1,name2}` names, comma-separated, no extension -- what
/// BibTeX itself parses out of the aux to know which `.bib` files to open.
function bibdataNames(auxText) {
  const names = new Set();
  for (const match of auxText.matchAll(/\\bibdata\{([^}]*)\}/g)) {
    for (const name of match[1].split(",")) {
      const trimmed = name.trim();
      if (trimmed) names.add(trimmed);
    }
  }
  return names;
}

function pathIn(tree, path) {
  return Boolean(tree?.texts?.[path] !== undefined || tree?.assets?.[path] !== undefined);
}

async function stageProjectFiles(engine, tree, extensions) {
  if (!tree) return;
  const all = { ...(tree.texts ?? {}), ...(tree.assets ?? {}) };
  for (const [path, contents] of Object.entries(all)) {
    if (!extensions.some((ext) => path.endsWith(ext))) continue;
    await ensureDirs(engine, path);
    await engine.writeFile(path, contents);
  }
}

/// Every file in `/work` this reply cares about: the fixed extension list
/// from section 2.4, plus any nested `.aux` a `\@input{...}` line in the main
/// aux points at (multi-file documents with `\include`), plus any `.aux` the
/// recorder (`.fls`) saw being read. No controller exposes a directory
/// listing, so this is a bounded set of `readFile` probes rather than an
/// actual scan -- a probe for a file that was never written just answers
/// null and is skipped.
async function collectOutputs(engine, stem, inputs) {
  const outputs = {};
  for (const ext of OUTPUT_EXTENSIONS) {
    const path = `${stem}.${ext}`;
    const bytes = await engine.readFile(path, true);
    if (bytes && bytes.length) outputs[path] = bytes;
  }
  const nested = new Set(outputs[`${stem}.aux`] ? nestedAuxPaths(outputs[`${stem}.aux`]) : []);
  for (const path of inputs ?? []) {
    if (path.endsWith(".aux")) nested.add(path);
  }
  for (const path of nested) {
    if (outputs[path]) continue;
    const bytes = await engine.readFile(path, true);
    if (bytes && bytes.length) outputs[path] = bytes;
  }
  return outputs;
}

const worker = new Worker2();

self.onmessage = async (event) => {
  const message = event.data;
  const { id } = message ?? {};
  try {
    const reply = await worker.handle(message);
    const { __transfer, ...rest } = reply;
    self.postMessage({ id, ...rest }, __transfer ?? []);
  } catch (error) {
    self.postMessage({ id, failed: String(error?.message || error) });
  }
};
