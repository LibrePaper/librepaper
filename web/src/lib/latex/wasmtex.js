// One nested WasmTex engine, driven.
//
// `worker.js` (the module worker the controller talks to) owns several of
// these -- pdfTeX, or XeTeX plus dvipdfm, or LuaTeX, plus BibTeX/BibTeX8/
// makeindex on demand -- and this file is the class for exactly one of them:
// a message queue with correlation, init, format preload, and the
// write/mkdir/read/run vocabulary every controller in
// WasmTex's `wasm-build/*-worker.js` (at the pinned source revision) answers
// to. Nothing from WasmTex's own `lib/` is imported; this was written
// against the controllers' `onmessage` dispatch and reply shapes directly
// (fetched and read from the pinned upstream release while building this).
//
// Two things about those controllers that are easy to get wrong and are the
// reason this file exists rather than one generic postMessage wrapper:
//
//   - Replies carry no request id. Every controller answers each command
//     with a message tagged `cmd`, and a `compilelatex`/`compilebibtex`/
//     `compilepdf`/... request always answers with `cmd: "compile"`
//     regardless of which one was sent. Because this driver only ever has
//     one command of a kind in flight (nothing here races two `run()`s),
//     a per-reply-cmd FIFO queue is enough to correlate correctly -- the
//     same trick WasmTex's own `postMessageWithResponse` uses.
//   - The controllers disagree with each other in small, real ways: BibTeX
//     and makeindex answer `result: "ok"/"error"` (not "failed") and never
//     set a numeric `status`; their `readfile` ignores the requested
//     encoding and always returns a UTF-8 string, unlike the TeX engines'
//     `readfile`, which honours `encoding: "binary"`. `run()` and
//     `readFile()` below normalise across that so callers do not have to
//     know which underlying binary they are talking to.
//
// The nested worker is created from a URL under the mirror -- never from a
// blob -- because Emscripten's glue finds its `.wasm` beside its own `.js`
// (relative to the script's own URL), and the controller's own
// `importScripts('wasmtex-<kind>-resolver-evidence.js')` resolves relative to
// that same URL too. A blob URL has no "beside it" for either to find.

import { fetchVerified } from "./resources.js";

const GZIP_MAGIC = [0x1f, 0x8b];

async function gunzip(bytes) {
  if (bytes.length < 2 || bytes[0] !== GZIP_MAGIC[0] || bytes[1] !== GZIP_MAGIC[1]) return bytes;
  if (typeof DecompressionStream === "undefined") return bytes;
  const stream = new Response(bytes).body.pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/// The in-worker filename a preloaded format is written under, for the two
/// engines that take theirs by file (XeTeX, LuaTeX) rather than by command
/// (pdfTeX's `loadformat`). Matches wasmtex's own `tex-fmt-engine.js`
/// (`ensureFormat`) exactly, because the compiled-in engine core looks for
/// this specific name via kpathsea.
const FMT_FILENAME = { xetex: "wasmtex-xetex.fmt", luatex: "wasmtex-luatex.fmt" };

/// Kinds whose controller understands the TeX-Live-resolution commands
/// (`preloadtexlive`, `loadbloom`, `preload404`, `flushcache`). BibTeX and
/// makeindex fetch nothing from TeX Live on their own (they only read the
/// files the host already staged) and their controllers do not implement
/// these commands at all -- sending them would just be ignored silently by
/// `onmessage`'s final `else`, but treating it as unsupported here keeps the
/// intent visible instead of relying on that silent ignore.
const RESOLVES_TEXLIVE = new Set(["pdftex", "xetex", "luatex", "dvipdfm"]);

/// Kinds whose controller answers `loadbundleindex`/`preloadbundle` (the
/// engine repository's `wasmtex-bundle-mode.js`, imported by every worker
/// but LuaTeX today). Kept in sync with `worker.js`'s own `BUNDLE_CAPABLE`.
const BUNDLE_CAPABLE = new Set(["pdftex", "xetex", "dvipdfm", "bibtex", "bibtex8", "makeindex"]);

export function createEngine({ kind, url, texliveUrl, format, release, onProgress, onDownload }) {
  return new WasmTexEngine(kind, url, texliveUrl, format, release, onProgress, onDownload);
}

class WasmTexEngine {
  constructor(kind, url, texliveUrl, format, release, onProgress, onDownload) {
    this.kind = kind;
    this.url = url;
    this.texliveUrl = texliveUrl;
    this.format = format ?? null;
    this.release = release ?? null;
    this.onProgress = onProgress;
    this.onDownload = onDownload;
    this.worker = null;
    // Reply queues keyed by the reply's own `cmd`. A command with no reply at
    // all (setmainfile, and every worker's `mkdir`... no, mkdir does reply)
    // never enqueues here.
    this.pending = new Map();
    this.madeDirs = new Set();
    // Resolver evidence for a package file (kpathsea format 26, `.sty`/
    // `.cls`) the bundle index itself says is not in the mirror -- SPEC-latex.md
    // "The resolver": with an index loaded, "absent" is a lookup, not a 404,
    // so a compile can fail with nothing in the log but a LaTeX
    // "File not found" for a name a reader cannot map to a package by
    // themselves. Collected between `run()` calls (see `run()` below) so the
    // failure this pass actually produced is what gets reported, not a
    // scrap left over from an earlier pass that happened to recover.
    this.mirrorAbsent = [];
  }

  /// Every command below waits on this queue, so a caller awaiting `run()`
  /// while `init()` is still in flight simply queues behind it -- there is
  /// no separate "not ready yet" state to check.
  _enqueue(replyCmd) {
    return new Promise((resolve, reject) => {
      const list = this.pending.get(replyCmd) ?? [];
      list.push({ resolve, reject });
      this.pending.set(replyCmd, list);
    });
  }

  _settle(replyCmd, message) {
    const list = this.pending.get(replyCmd);
    if (!list || list.length === 0) return false;
    const waiter = list.shift();
    if (list.length === 0) this.pending.delete(replyCmd);
    waiter.resolve(message);
    return true;
  }

  _rejectAll(error) {
    for (const list of this.pending.values()) {
      for (const waiter of list) waiter.reject(error);
    }
    this.pending.clear();
  }

  /// A command the controller answers: post it, then wait for the next
  /// reply tagged `replyCmd`. `transfer` is for the one caller
  /// (`preloadTexlive`) that hands over an ArrayBuffer it will not reuse.
  async _ask(message, replyCmd, transfer) {
    if (!this.worker) throw new Error(`${this.kind} engine is not initialized`);
    const wait = this._enqueue(replyCmd);
    if (transfer?.length) this.worker.postMessage(message, transfer);
    else this.worker.postMessage(message);
    return wait;
  }

  /// A command none of the controllers answer at all (`setmainfile`,
  /// `settexliveurl`, `loadbloom`, `grace`). Waiting for a reply that never
  /// arrives would hang every future compile.
  _tell(message, transfer) {
    if (!this.worker) throw new Error(`${this.kind} engine is not initialized`);
    if (transfer?.length) this.worker.postMessage(message, transfer);
    else this.worker.postMessage(message);
  }

  async init() {
    if (this.worker) return;
    await new Promise((resolve, reject) => {
      const worker = new Worker(this.url);
      worker.onmessage = (event) => this._onmessage(event.data, resolve, reject);
      worker.onerror = (event) => {
        const error = new Error(`${this.kind} engine worker error: ${event.message || event}`);
        this._rejectAll(error);
        reject(error);
      };
      this.worker = worker;
    });
    this._tell({ cmd: "settexliveurl", url: this.texliveUrl });
    if (this.kind === "pdftex") {
      // Preamble snapshots are a wasmtex-side speed trick (a cached format
      // for the unchanged part of the document); the controller expects a
      // reply, so this is one round trip before the engine is usable.
      await this._ask({ cmd: "setpreamblesnapshot", enabled: false }, "setpreamblesnapshot");
    }
    if (this.format) await this._loadFormat();
  }

  async _loadFormat() {
    const response = await fetchVerified(this.release, this.format.url, {
      sha256: this.format.sha256,
      size: this.format.size,
    });
    let bytes = new Uint8Array(await response.arrayBuffer());
    if (this.format.gz) bytes = await gunzip(bytes);
    this.onProgress?.({ done: 1, total: 1, scope: "engine and format" });
    if (this.kind === "pdftex") {
      // pdfTeX takes its format by command: the controller keeps it in a JS
      // variable (`self._fmtData`), not on the in-memory filesystem.
      const copy = bytes.slice().buffer;
      await this._ask({ cmd: "loadformat", data: copy }, "loadformat", [copy]);
      return;
    }
    const filename = FMT_FILENAME[this.kind];
    if (!filename) return; // dvipdfm/bibtex/bibtex8/makeindex have no format
    // XeTeX and LuaTeX take theirs by file, written before the first pass --
    // there is no "loadformat" command in either controller. Unlike
    // pdfTeX's `self._fmtData`, this file lives in `/work`, which
    // `flushcache` (the only way to clear a stale snapshot -- see
    // `flushCache` below) wipes entirely; the bytes are kept here so every
    // `flushCache()` can rewrite the file straight back.
    this._fmtFilename = filename;
    this._fmtBytes = bytes;
    await this.writeFile(filename, bytes);
  }

  _onmessage(message, resolveInit, rejectInit) {
    if (!message || typeof message !== "object") return;
    if (!message.cmd) {
      // The one reply every controller sends with no `cmd`: Emscripten's
      // postRun, meaning the engine finished starting.
      if (message.result === "ok") resolveInit();
      else rejectInit(new Error(`${this.kind} engine failed to start`));
      return;
    }
    if (message.cmd === "downloading") {
      // `bundle`/`size` are only present once a release ships a bundle
      // index (SPEC-latex.md "The resolver": "the worker's downloading
      // message gains the bundle name and size"); legacy per-file mode
      // sends `file` alone, as before.
      this.onDownload?.({ file: message.file, bundle: message.bundle, size: message.size });
      return;
    }
    if (message.cmd === "resolverready") return;
    if (message.cmd === "resolver") {
      // Resolver telemetry from `*-resolver-evidence.js`. Nothing here reads
      // most of it (that file feeds wasmtex's own diagnostics, which this
      // driver does not reproduce), except the one case a failed compile
      // needs named: a `.sty`/`.cls` (format 26) the bundle index itself
      // says is absent, per docs/specs "Precise failure messages".
      const evidence = message.evidence;
      if (
        evidence &&
        evidence.outcome === "mirror-absent" &&
        evidence.format === 26 &&
        Array.isArray(evidence.attempts) &&
        evidence.attempts.some((attempt) => attempt?.source === "bundle-index")
      ) {
        this.mirrorAbsent.push(evidence.requestedName);
      }
      return;
    }
    if (message.cmd === "workererror") {
      const error = new Error(message.errorMessage || "engine worker error");
      this._rejectAll(error);
      return;
    }
    this._settle(message.cmd, message);
  }

  async mkdir(path) {
    // No controller has `mkdir -p`; each ancestor is created in turn and an
    // "already exists" answer is not a reason to stop, matching the same
    // pattern `swiftlatex.js` uses for the same reason.
    const parts = path.split("/").filter(Boolean);
    let built = "";
    for (const part of parts) {
      built = built ? `${built}/${part}` : part;
      if (this.madeDirs.has(built)) continue;
      this.madeDirs.add(built);
      await this._ask({ cmd: "mkdir", url: built }, "mkdir").catch(() => {});
    }
  }

  /// `bytes` must arrive as bytes, never as a string that happens to look
  /// right: a `Uint8Array` is sent as-is (structured clone preserves it
  /// exactly), and a `string` is sent as-is too, which every controller
  /// writes with `FS.writeFile(path, content)` -- Emscripten's own FS
  /// accepts either and only a `Uint8Array` guarantees encoding-independent
  /// bytes, which matters for a `.bbl`/`.bib`/font file that is not ASCII.
  async writeFile(path, bytesOrString) {
    await this._ask({ cmd: "writefile", url: path, src: bytesOrString }, "writefile");
  }

  /// `binary` defaults true because most callers want bytes; the exception
  /// (an .aux read back to hand to BibTeX, `main.tex` itself) can ask for
  /// `binary: false` and get a string back directly. BibTeX's and
  /// makeindex's controllers ignore the requested encoding and always
  /// decode as UTF-8 -- for those, a `binary: true` request still gets a
  /// string back here and is re-encoded with `TextEncoder`, which is exact
  /// as long as the underlying bytes were valid UTF-8 (true for every
  /// project `.bib`/`.aux` this pipeline writes, since the tree reader
  /// already requires UTF-8 source).
  async readFile(path, binary = true) {
    const reply = await this._ask(
      { cmd: "readfile", url: path, encoding: binary ? "binary" : "utf8" },
      "readfile",
    );
    if (reply.result !== "ok") return null;
    const data = reply.data;
    if (data == null) return null;
    if (!binary) return typeof data === "string" ? data : new TextDecoder().decode(new Uint8Array(data));
    if (typeof data === "string") return new TextEncoder().encode(data);
    return new Uint8Array(data);
  }

  setMainFile(path) {
    this._tell({ cmd: "setmainfile", url: path });
  }

  /// One pass. `cmd` is the controller's own command name
  /// (`compilelatex`/`compilepdf`/`compilebibtex`/`compilebibtex8`/
  /// `compilemakeindex`); `extra` carries the `url` field BibTeX/makeindex
  /// need (the job stem) since they take it on the request rather than via
  /// a prior `setmainfile`.
  async run(cmd, extra = {}) {
    this.mirrorAbsent = [];
    const reply = await this._ask({ cmd, ...extra }, "compile");
    // pdfTeX/XeTeX/dvipdfm/LuaTeX: result "ok"/"failed" plus a numeric exit
    // status (0 and 1 both "compiled", 1 meaning warnings). BibTeX/makeindex:
    // result "ok"/"error", no status at all -- treated as exit 0 or 1.
    const ok = reply.result === "ok" && (reply.status === undefined || reply.status === 0 || reply.status === 1);
    return {
      status: typeof reply.status === "number" ? reply.status : ok ? 0 : 1,
      ok,
      pdf: reply.pdf ? new Uint8Array(reply.pdf) : null,
      synctex: reply.synctex ? new Uint8Array(reply.synctex) : null,
      log: reply.log || "",
      inputs: Array.isArray(reply.inputFiles) ? reply.inputFiles : null,
      mirrorAbsent: this.mirrorAbsent.slice(),
    };
  }

  /// `format`/`name` are the kpathsea format code and bare filename from a
  /// `texlive.files` key (`pdftex/26/amsmath.sty` -> `format: 26, name:
  /// "amsmath.sty"`), exactly how the controllers key their own cache.
  ///
  /// Fire-and-forget, deliberately: only pdfTeX's controller answers this
  /// command (`{result:"ok", cmd:"preloadtexlive", msgId}`) -- XeTeX's and
  /// dvipdfm's `onmessage` handle it (write the file, update their cache)
  /// but never `postMessage` afterwards, confirmed by reading both. Awaiting
  /// a reply here hung `ensureEngine` forever on those two kinds, on the
  /// very first entry of the initial resource set. Message order to one
  /// worker is preserved regardless of whether the host awaits a reply, so
  /// not waiting costs nothing even for pdfTeX.
  preloadTexlive(format, name, bytes) {
    if (!RESOLVES_TEXLIVE.has(this.kind)) return;
    const copy = bytes.slice().buffer;
    this._tell({ cmd: "preloadtexlive", format, filename: name, data: copy }, [copy]);
  }

  loadBloom(bytes) {
    if (!RESOLVES_TEXLIVE.has(this.kind)) return;
    this._tell({ cmd: "loadbloom", data: bytes.slice().buffer });
  }

  /// SPEC-latex.md "The index": loads the whole-mirror bundle index once,
  /// replacing the bloom filter and the per-name 404/preload warmups for a
  /// release that ships one. Unlike `loadBloom`/`preload404`
  /// (fire-and-forget), this command answers -- the controller awaits it so
  /// a first compile never races the worker's own Cache Storage preload.
  async loadBundleIndex(bytes) {
    // Every worker but LuaTeX now importScripts()s `wasmtex-bundle-mode.js`
    // and answers this command; a kind that does not is a no-op here so a
    // caller need not special-case it (worker.js's `ensureEngine` already
    // gates the call on `BUNDLE_CAPABLE`, but this stays defensive on its
    // own since nothing else enforces that from this side).
    if (!BUNDLE_CAPABLE.has(this.kind)) return null;
    const copy = bytes.slice().buffer;
    return this._ask({ cmd: "loadbundleindex", data: copy }, "loadbundleindex", [copy]);
  }

  /// XeTeX only: sends the inflated `icudt68l.dat` bytes (the release ships
  /// them gzipped) so ICU-backed lookups -- font-by-name resolution, Unicode
  /// data the engine core needs -- work in bundle mode, where nothing named
  /// `icudt68l.dat` exists on the endpoint for the worker to fetch by name.
  async loadIcuData(bytes) {
    if (this.kind !== "xetex") return null;
    const copy = bytes.slice().buffer;
    return this._ask({ cmd: "loadicudata", data: copy }, "loadicudata", [copy]);
  }

  preload404(entries) {
    // Fire-and-forget, deliberately: pdfTeX's controller answers this
    // command (`cmd: "preload404"`), but XeTeX's and dvipdfm's do not reply
    // to it at all (confirmed by reading their `onmessage` -- the loop that
    // populates their negative cache has no `postMessage` after it), so
    // awaiting a reply here would hang forever on those two kinds.
    if (!RESOLVES_TEXLIVE.has(this.kind) || !entries?.length) return;
    this._tell({ cmd: "preload404", entries });
  }

  /// A fresh `/work` for the next snapshot. BibTeX and makeindex have no
  /// `flushcache` command at all (their controllers never accumulate project
  /// state worth clearing between the one-shot runs they're used for), so
  /// this is a no-op for those two -- `worker.js` always writes their inputs
  /// fresh before every run anyway.
  async flushCache() {
    if (this.kind === "bibtex" || this.kind === "bibtex8" || this.kind === "makeindex") return;
    this._tell({ cmd: "flushcache" });
    this.madeDirs.clear();
    // pdfTeX's format survives `flushcache` (it is a JS variable, never on
    // `/work`); XeTeX's and LuaTeX's do not, since `_loadFormat` wrote
    // theirs as an ordinary file -- rewrite it now so the engine is not left
    // formatless for the compile `stage` is about to run.
    if (this._fmtFilename && this._fmtBytes) {
      await this.writeFile(this._fmtFilename, this._fmtBytes);
    }
  }

  terminate() {
    this.worker?.terminate();
    this.worker = null;
    this._rejectAll(new Error(`${this.kind} engine terminated`));
  }
}
