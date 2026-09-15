// LaTeX, compiled in this browser.
//
// This controller owns exactly
// one module worker running the browser engine, speaks the worker message protocol to it,
// and runs browser Biber when needed. `latex/jobs.js` gives every compile its identity, `latex/
// bibliography.js` reads the real aux/bcf/log a pass produced rather than
// guessing from source text, and `latex/status.js` is the store the reader's
// badge subscribes to.
//
// Three rules from the spec shape almost every decision below:
//
//   - A document that does not compile is an ordinary state of an editor, not
//     an error of this module's. `compile()` resolves with `ok: false` and a
//     `failure`; it rejects only for a job that was canceled or superseded.
//   - A result carries its `job` back unchanged, and a result whose
//     generation is older than the newest one already resolved must never
//     become `status().lastResult` -- an edit made and then undone within a
//     debounce window must not flicker the preview backwards.
//   - Browser Biber failures are reported immediately and are never retried
//     through another backend.
//
// `latex/worker.js` is loaded through a constructor tests can replace.
// `_testing.inject` is how `latex-controller.mjs` supplies fakes; see its doc
// comment below. `latex/engine.js`
// is different: it is a small, dependency-free module, so it is imported
// statically like any ordinary dependency.

import * as jobsMod from "./latex/jobs.js";
import * as bibliography from "./latex/bibliography.js";
import * as statusStore from "./latex/status.js";
import * as logMod from "./latex/log.js";
import * as engineMod from "./latex/engine.js";
import { snapshotDigest } from "./tree-digest.js";
import { maybeBytes as toBytes } from "./bytes.js";
import { named } from "./latex/errors.js";

export const DEBOUNCE = 1500;
export const DEFAULT_BASE = "https://latex.librepaper.workers.dev/";

/// The whole job's time budget and the bounded pass count SPEC "Browser
/// compilation controller" asks for. Both are exceeded as a reported
/// `failure.kind: "timeout"`.
export const DEADLINE_MS = 240_000;
export const MAX_PASSES = 8;

let base = DEFAULT_BASE;

// Swappable seams. Production leaves every one of these at its default; only
// `_testing.inject` (used by `latex-controller.mjs`) ever changes them, which
// is what lets this file's own logic run under Node with no worker, no
// network and no real local app anywhere in reach.
let WorkerClass = typeof Worker !== "undefined" ? Worker : null;
let biberOverride;
let biberModule;
let resourcesOverride;
let fetchImpl = (...args) => fetch(...args);
let nowImpl = () => Date.now();

let manifest = null;
let manifestPromise = null;

let currentProject = null;
let currentSettings = { engine: "auto" };

// The live worker and its RPC bookkeeping. `configuredRelease` is the
// release id the worker last accepted a `configure` for; a different release
// on the next compile retires it rather than reusing a mismatched engine.
let worker = null;
let configuredRelease = null;
let nextCallId = 1;
const pendingCalls = new Map(); // id -> {resolve, reject}

// The queue: at most one running, one queued, and the queued one is always
// the newest tree (SPEC "Browser compilation controller"). `jobGeneration`
// is the "monotonically increasing local generation" section "Project
// configuration and identity" asks every Job to carry; it advances exactly
// once per job that actually starts, which is also the only thing compared
// to decide whether a result may become `status().lastResult`.
let running = false;
let queued = null; // { tree, manual, waiting: [], failing: [] }
let activeToken = null; // { cancelled: boolean } for the job currently running
let activeCallbacks = null; // { waiting, failing } for that same job
let jobGeneration = 0;
let newestResolvedGeneration = -1;

// Bibliography state persists for the whole session; `bibCache` is keyed by
// `bibliography.identity()`'s hash,
// so a citation, database or style change simply misses it rather than
// needing an explicit invalidation path.
const bibCache = new Map(); // identity -> BiberResult
let lastStaged = null; // { project, inputs, bibIdentity, generated }

/// Routing decisions, on the console, when `localStorage["librepaper-latex-debug"]`
/// is set: the one way to see why a compile went where it went without a
/// debugger attached to a worker.
function trace(...words) {
  try {
    if (typeof localStorage !== "undefined" && localStorage.getItem("librepaper-latex-debug")) console.debug("latex:", ...words);
  } catch {
    /* storage refused: nothing to trace to */
  }
}

/// The mirror base as an absolute URL, which is what every backend that
/// fetches from the mirror resolves relative paths against.
function absoluteBase() {
  return typeof location !== "undefined" ? new URL(base, location.href).href : base;
}

class WorkerDied extends Error {
  constructor(message) {
    super(message);
    this.name = "WorkerDied";
  }
}

function supersededError() {
  return named("Superseded", "Superseded");
}

/// Points this module at the direct HTTPS mirror advertised by `/api/config`.
export function at(url) {
  if (url && url !== base) {
    base = url.endsWith("/") ? url : url + "/";
    manifest = null;
    manifestPromise = null;
    retireWorker();
  }
  return base;
}

function retireWorker() {
  if (worker) {
    try {
      worker.terminate();
    } catch {
      /* already gone */
    }
  }
  worker = null;
  configuredRelease = null;
  for (const [, call] of pendingCalls) call.reject(supersededError());
  pendingCalls.clear();
}

async function loadManifest() {
  if (manifest) return manifest;
  if (!manifestPromise) {
    manifestPromise = fetchImpl(base + "manifest.json").then(async (response) => {
      if (!response.ok) throw new Error(`no LaTeX mirror at ${base} (${response.status})`);
      manifest = await response.json();
      return manifest;
    });
  }
  try {
    return await manifestPromise;
  } finally {
    // A failed fetch must not poison the module: the next compile tries
    // again rather than repeating a network error forever from cache.
    if (!manifest) manifestPromise = null;
  }
}

/// Called by the reader when a document opens. Resets the queue, the
/// session route and every per-project cache when the project itself
/// changes; a settings-only reconfigure of the same project is `setSettings`.
export function configure({ project, settings: nextSettings, mayCompile = true } = {}) {
  const changedProject = project !== currentProject;
  currentProject = project;
  currentSettings = normalizeSettings(nextSettings);
  delete currentSettings.release;
  if (changedProject) {
    cancel();
    jobGeneration = 0;
    newestResolvedGeneration = -1;
    bibCache.clear();
    lastStaged = null;
  }
  statusStore.set({
    phase: "idle",
    engine: currentSettings.engine === "auto" ? null : currentSettings.engine,
    release: null,
  });
  void mayCompile; // reserved for the reader's own read-only gating, not this module's
}

export function settings() {
  return currentSettings;
}

function normalizeSettings(next = {}) {
  const backend = ["auto", "browser", "local"].includes(next.backend) ? next.backend : "auto";
  const tool = typeof next.tool === "string" && next.tool ? next.tool : "tex";
  const engine = ["auto", "pdflatex", "xelatex", "lualatex"].includes(next.engine) ? next.engine : "auto";
  return { engine, backend, tool, output: next.output || "html", preset: typeof next.preset === "string" ? next.preset : "", options: next.options && typeof next.options === "object" ? { ...next.options } : {} };
}

/// An engine change clears the bibliography cache and starts a fresh routing
/// session rather than trying to reconcile artifacts across toolchains.
export function setSettings(next) {
  currentSettings = normalizeSettings({ ...currentSettings, ...next });
  delete currentSettings.release;
  bibCache.clear();
  lastStaged = null;
  configuredRelease = null; // the next compile must (re)configure the worker for the new release
  cancel();
  statusStore.set({
    route: "browser",
    engine: currentSettings.engine === "auto" ? null : currentSettings.engine,
    release: null,
  });
}

export async function releases() {
  const data = await loadManifest();
  return {
    default: data.default_release,
    current: data.default_release,
    available: Object.entries(data.releases || {}).map(([id, entry]) => ({
      id,
      texlive: entry.texlive,
      kernel: entry.kernel,
      sizes: entry.sizes,
    })),
  };
}

// --- Engine selection -------------------------------------------------------
//
// Section 2.3's algorithm now lives in `latex/engine.js` (package B1); this
// is a one-line delegation. `engine.js` is a pure, dependency-free module
// (no imports of its own), so importing it statically here carries none of
// the "must load before the file exists" risk the worker/local/resources
// modules do -- it is safe to load unconditionally.
export function resolveEngine(tree, settingsArg = currentSettings) {
  return engineMod.resolveEngine(tree, settingsArg);
}

// --- The worker RPC (section 2.4) ------------------------------------------

/// "amsmath, 1.2 MB" -- the one place a byte count is put in front of a
/// reader (SPEC-latex.md "The resolver"), so it stays a plain decimal
/// megabyte figure rather than binary units a document author has no reason
/// to know.
function formatBytes(bytes) {
  return `${(bytes / 1e6).toFixed(1)} MB`;
}

function attachHandlers(target) {
  target.onmessage = (event) => {
    const message = event.data;
    if (!message) return;
    if (message.cmd === "progress") {
      statusStore.set({ progress: { done: message.done, total: message.total, scope: message.scope } });
      return;
    }
    if (message.cmd === "downloading") {
      // SPEC-latex.md "The resolver": "the host's progress indicator reports
      // 'amsmath, 1.2 MB' instead of a stream of file names" -- only present
      // once a release ships a bundle index; legacy per-file mode sends
      // `file` alone and stays silent here, as before.
      if (message.bundle) {
        const scope = typeof message.size === "number" ? `${message.bundle}, ${formatBytes(message.size)}` : message.bundle;
        statusStore.set({ progress: { done: 0, total: 0, scope } });
      }
      return;
    }
    const call = pendingCalls.get(message.id);
    if (!call) return;
    pendingCalls.delete(message.id);
    if (message.failed) call.reject(new Error(message.failed));
    else call.resolve(message);
  };
  target.onerror = (event) => {
    if (worker !== target) return;
    const error = new WorkerDied(event?.message || "the compiler worker failed");
    worker = null;
    configuredRelease = null;
    for (const [, call] of pendingCalls) call.reject(error);
    pendingCalls.clear();
    try {
      target.terminate();
    } catch {
      /* already gone */
    }
  };
}

function call(target, cmd, payload, { timeoutMs = 0 } = {}) {
  return new Promise((resolve, reject) => {
    const id = nextCallId++;
    // An engine that never answers -- a worker wedged inside a synchronous
    // fetch, a pass that loops -- would otherwise hold the queue for ever:
    // the job's remaining budget bounds every call, and a call that outlives
    // it retires the worker so the next compile starts from a clean engine.
    let timer = null;
    if (timeoutMs > 0) {
      timer = setTimeout(() => {
        if (!pendingCalls.has(id)) return;
        pendingCalls.delete(id);
        const error = named("WorkerTimeout", `the compiler did not answer ${cmd} within its time budget`);
        if (worker === target) retireWorker();
        reject(error);
      }, timeoutMs);
    }
    const settle = (fn) => (value) => {
      if (timer) clearTimeout(timer);
      fn(value);
    };
    pendingCalls.set(id, { resolve: settle(resolve), reject: settle(reject) });
    try {
      target.postMessage({ id, cmd, ...payload });
    } catch (error) {
      if (timer) clearTimeout(timer);
      pendingCalls.delete(id);
      reject(error);
    }
  });
}

async function ensureWorker(releaseEntry, manifestFormat) {
  if (worker && configuredRelease === releaseEntry.id) return worker;
  if (worker) {
    try {
      worker.terminate();
    } catch {
      /* already gone */
    }
    worker = null;
    configuredRelease = null;
  }
  // The literal `new Worker(new URL(..., import.meta.url), { type: "module" })`
  // is what Vite recognises and bundles as a worker of its own; behind a
  // variable it would only copy the file verbatim, and the worker's bare
  // imports would then 404 in production. The injected class is for the
  // checks alone, so it takes the other branch.
  const injected = WorkerClass && (typeof Worker === "undefined" || WorkerClass !== Worker) ? WorkerClass : null;
  if (!injected && typeof Worker === "undefined") throw new Error("no Worker implementation is available");
  const target = injected
    ? new injected(new URL("./latex/worker.js", import.meta.url), { type: "module" })
    : new Worker(new URL("./latex/worker.js", import.meta.url), { type: "module" });
  attachHandlers(target);
  worker = target;
  // The worker resolves every engine and package URL against this, and a
  // The worker receives the configured absolute mirror URL, so all manifest
  // paths resolve against the direct mirror rather than an app route.
  await call(target, "configure", { base: absoluteBase(), release: releaseEntry, format: manifestFormat });
  configuredRelease = releaseEntry.id;
  return target;
}

function classifyWorkerError(error, fallbackKind) {
  // "Worker death -> reject the active job (failure.kind='init' result...)"
  // regardless of which command was in flight when it happened.
  return error?.name === "WorkerDied" ? "init" : fallbackKind;
}

// --- Backend seams (browser Biber)
// ----------------------------

async function getResources() {
  if (resourcesOverride !== undefined) return resourcesOverride;
  try {
    return await import("./latex/resources.js");
  } catch {
    return null;
  }
}

// --- Small helpers over trees and worker outputs ----------------------------

function stemOf(main) {
  return String(main || "main.tex").split("/").pop().replace(/\.[^./]+$/, "");
}

function normalizeOutputs(raw) {
  const out = {};
  for (const [path, bytes] of Object.entries(raw || {})) {
    out[path] = bytes instanceof Uint8Array ? bytes : bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : bytes;
  }
  return out;
}

function fileBytes(tree, path) {
  if (tree?.texts && Object.prototype.hasOwnProperty.call(tree.texts, path)) {
    return new TextEncoder().encode(String(tree.texts[path]));
  }
  if (tree?.assets && Object.prototype.hasOwnProperty.call(tree.assets, path)) {
    return toBytes(tree.assets[path]);
  }
  return null;
}

function pickGenerated(outputs) {
  const generated = {};
  for (const [path, bytes] of Object.entries(outputs)) {
    if (/\.(aux|bbl|ind|toc)$/i.test(path)) generated[path] = bytes;
  }
  return generated;
}

function treePaths(tree) {
  return [...Object.keys(tree?.texts || {}), ...Object.keys(tree?.assets || {})];
}

function baseProvenance(engine, release) {
  return { backend: "browser", bibliography: null, engine, release, tools: {} };
}

function buildResult({ job, attempts, startedAt, ok, pdf = null, synctex = null, log = "", diagnostics, failure, provenance }) {
  return {
    ok,
    pdf: pdf || null,
    synctex: synctex || null,
    log: log || "",
    attempts,
    diagnostics: diagnostics || (log ? logMod.parse(log, { main: "", paths: [] }) : []),
    seconds: (nowImpl() - startedAt) / 1000,
    job,
    provenance,
    failure: failure || null,
  };
}

/// When the bundle index says a requested `.sty`/`.cls` is absent from the
/// browser mirror, name it instead of the generic compile failure.
async function mirrorAbsentMessage(mirrorAbsent) {
  const names = [...new Set(Array.isArray(mirrorAbsent) ? mirrorAbsent : [])];
  if (!names.length) return null;
  const named = names.length === 1 ? `"${names[0]}"` : `"${names[0]}" and ${names.length - 1} more`;
  return `Package ${named} is not available in the browser mirror.`;
}

// What this browser cannot do, in the document's own terms.
//
// The engine's error for a document that wants LuaTeX or shell-escape is a
// true statement about a macro and a useless one about the situation:
// "Undefined control sequence" for \\directlua, "-shell-escape" for minted.
// LibrePaper builds LaTeX in the browser and nowhere else, so these are
// permanent limits of this document here, not transient errors -- saying so
// is more use than a control sequence name.
function unsupportedReason(source, log) {
  if (engineMod.needsLuaTeX(source)) {
    return "This document needs LuaTeX, which LibrePaper's browser compiler does not include. LuaTeX-only packages and \\directlua cannot be built here.";
  }
  if (/shell-escape|\\write18|runsystem\(/i.test(log)) {
    return "This document asks LaTeX to run another program (shell-escape), which a browser cannot do. Packages like minted, svg and gnuplot backends need it; a pre-rendered figure does not.";
  }
  return "";
}

async function handleBrowserFailure({ job, tree, engine, releaseId, attempts, startedAt, kind, message, signal }) {
  // The failure's own words are the reason; the log is the engine's, from
  // the attempt that just failed, and the diagnostics are read out of it so
  // a missing package or an undefined control sequence lands in the gutter
  // rather than behind a generic sentence. A limit this compiler simply does
  // not have replaces that reason: the engine's complaint is a symptom.
  const lastLog = [...attempts].reverse().find((one) => one.stage === "browser")?.log || "";
  const unsupported = unsupportedReason(tree?.texts?.[tree?.main] ?? "", lastLog);
  return buildResult({
    job,
    attempts,
    startedAt,
    ok: false,
    log: lastLog,
    diagnostics: lastLog ? logMod.parse(lastLog, { main: tree.main, paths: treePaths(tree) }) : [],
    failure: {
      kind: unsupported ? "unsupported" : kind,
      message: unsupported || message,
      stage: "browser",
    },
    provenance: baseProvenance(engine, releaseId),
  });
}

// --- The compile sequence (SPEC "Browser compilation controller" /
// interfaces.md section 2.8) -------------------------------------------------

async function runCompile({ tree, jobGeneration: generationAtStart, token, startedAt }) {
  const checkpoint = () => {
    if (token.cancelled) throw supersededError();
  };

  const engine = resolveEngine(tree, currentSettings);

  let manifestData;
  try {
    manifestData = await loadManifest();
  } catch (error) {
    checkpoint();
    const inputs = await snapshotDigest(tree).catch(() => "");
    const job = await jobsMod.makeJob({
      project: currentProject,
      generation: generationAtStart,
      tree,
      inputs,
      engine,
      release: manifestData?.default_release || "unknown",
    });
    return buildResult({
      job,
      attempts: [],
      startedAt,
      ok: false,
      failure: { kind: "resources", message: String(error?.message || error), stage: "browser" },
      provenance: baseProvenance(engine, null),
    });
  }
  checkpoint();

  const releaseId = manifestData.default_release;
  const inputs = await snapshotDigest(tree);
  const job = await jobsMod.makeJob({ project: currentProject, generation: generationAtStart, tree, inputs, engine, release: releaseId });
  checkpoint();

  const attempts = [];
  const releaseEntry = manifestData.releases?.[releaseId];
  if (!releaseEntry) {
    return buildResult({
      job,
      attempts,
      startedAt,
      ok: false,
      failure: {
        kind: "resources",
        message: releaseId
          ? `LaTeX release "${releaseId}" is not available in this deployment's mirror.`
          : "This deployment's LaTeX mirror has no default engine release. The operator must build and deploy the mirror from the wasm-latex repository (make mirror, then make push there), or configure a working --latex-mirror.",
        stage: "browser",
      },
      provenance: baseProvenance(engine, releaseId),
    });
  }
  // LuaLaTeX is never selected for the author (see `engine.js`'s `detect`);
  // reaching here means the setting or a `% !TEX engine` directive asked for
  // it by name. No release ships LuaTeX, so say that rather than implying a
  // later release might, or that the document is at fault.
  if (engine === "lualatex" && !releaseEntry.engines?.luatex) {
    return buildResult({
      job, attempts, startedAt, ok: false,
      failure: {
        kind: "unsupported",
        message: "LibrePaper's browser compiler does not include LuaTeX. Choose pdfLaTeX or XeLaTeX, or remove the LuaLaTeX directive.",
        stage: "browser",
      },
      provenance: baseProvenance(engine, releaseId),
    });
  }
  statusStore.set({ phase: "loading", message: "Loading browser compiler", backend: "browser", release: releaseId, engine, progress: null });

  let target;
  try {
    target = await ensureWorker(releaseEntry, manifestData.format);
  } catch (error) {
    checkpoint();
    return handleBrowserFailure({
      job,
      tree,
      engine,
      releaseId,
      attempts,
      startedAt,
      signal: token.abort.signal,
      kind: classifyWorkerError(error, "init"),
      message: String(error?.message || error),
    });
  }
  checkpoint();

  statusStore.set({ phase: "compiling", message: "Compiling in browser", backend: "browser", release: releaseId, engine });

  const generated = lastStaged && lastStaged.project === currentProject ? lastStaged.generated : {};
  try {
    await call(target, "stage", { engine, tree, generated });
  } catch (error) {
    checkpoint();
    return handleBrowserFailure({
      job,
      tree,
      engine,
      releaseId,
      attempts,
      startedAt,
      signal: token.abort.signal,
      kind: classifyWorkerError(error, "resources"),
      message: String(error?.message || error),
    });
  }
  checkpoint();

  const stem = stemOf(tree.main);
  let outputs = {};
  let finalLog = "";
  let lastPdf = null;
  let lastSynctex = null;
  let bibliographyProvenance = null;
  let bibliographyTools = {};
  let passes = 0;
  const deadlineAt = startedAt + DEADLINE_MS;

  for (;;) {
    checkpoint();
    if (nowImpl() > deadlineAt) {
      attempts.push({ stage: "browser", backend: "browser", ok: false, log: finalLog, reason: "timeout" });
      return handleBrowserFailure({
        job,
        tree,
        engine,
        releaseId,
        attempts,
        startedAt,
        signal: token.abort.signal,
        kind: "timeout",
        message: "The compile exceeded its time budget.",
      });
    }
    passes += 1;

    let reply;
    try {
      reply = await call(target, "tex", { engine, main: tree.main }, { timeoutMs: Math.max(1000, deadlineAt - nowImpl()) });
    } catch (error) {
      checkpoint();
      const timedOut = error?.name === "WorkerTimeout";
      attempts.push({ stage: "browser", backend: "browser", ok: false, log: finalLog, reason: timedOut ? "timeout" : "tex" });
      return handleBrowserFailure({
        job,
        tree,
        engine,
        releaseId,
        attempts,
        startedAt,
        signal: token.abort.signal,
        kind: timedOut ? "timeout" : classifyWorkerError(error, "tex"),
        message: String(error?.message || error),
      });
    }
    checkpoint();

    outputs = { ...outputs, ...normalizeOutputs(reply.outputs) };
    finalLog = reply.log || "";
    lastPdf = toBytes(reply.pdf);
    lastSynctex = toBytes(reply.synctex);

    if (!(reply.status === 0 || reply.status === 1) || !lastPdf) {
      attempts.push({ stage: "browser", backend: "browser", ok: false, log: finalLog, reason: "tex" });
      return handleBrowserFailure({
        job,
        tree,
        engine,
        releaseId,
        attempts,
        startedAt,
        signal: token.abort.signal,
        kind: "tex",
        message: (await mirrorAbsentMessage(reply.mirrorAbsent)) || "The document failed to compile.",
      });
    }

    const inspected = bibliography.inspect({ stem, outputs, log: finalLog, tree });

    if (inspected.makeindex) {
      try {
        const idx = await call(target, "makeindex", { stem });
        if (idx.ind) outputs[`${stem}.ind`] = toBytes(idx.ind);
      } catch {
        // A missing/failed index is visible in the PDF and the log; it does
        // not abort a compile that otherwise produced a usable document.
      }
    }

    // A .bib edit need not make TeX request Biber: compare the actual inputs
    // on the first pass even when an earlier BBL is already staged.
    const useBiber = inspected.biber || (releaseEntry.engines?.biber && inspected.bcf && passes === 1);
    let bibliographyChanged = false;
    if (useBiber || inspected.bibtex || inspected.bibtex8) {
      const kind = useBiber ? "biber" : inspected.bibtex8 ? "bibtex8" : "bibtex";
      const controlBytes = useBiber ? inspected.bcf : outputs[`${stem}.aux`];
      const files = {};
      for (const path of [...inspected.bibFiles, ...inspected.styleFiles, ...inspected.configFiles]) {
        const bytes = fileBytes(tree, path);
        if (bytes) files[path] = bytes;
      }
      const identity = await bibliography.identity({ kind, controlBytes, files, engine, release: releaseId, tool: kind });

      let bibResult = bibCache.get(identity) || null;
      if (bibResult) {
        bibliographyProvenance = kind === "biber" ? "browser-biber" : "bibtex";
        if (bibResult.tool?.version) bibliographyTools = { ...bibliographyTools, biber: bibResult.tool.version };
      } else if (useBiber) {
        const request = { job, stem, main: tree.main, bcf: controlBytes, files, identity };
        statusStore.set({ phase: "browser-biber", message: "Updating bibliography in browser", backend: "browser" });
        try {
          const backend = biberOverride !== undefined ? biberOverride : (biberModule ||= await import("./latex/biber.js"));
          bibResult = await backend.runBiber(request, {
            base: absoluteBase(), release: releaseEntry, signal: token.abort.signal,
            onProgress: progress => { if (!token.cancelled) statusStore.set({ progress }); },
          });
        } catch (error) {
          if (token.cancelled || error?.name === "AbortError") throw supersededError();
          bibResult = { ok: false, error: String(error?.message || error), blg: String(error), tool: { backend: "browser" } };
        }
        attempts.push({ stage: "browser-biber", backend: "browser", ok: bibResult.ok, log: bibResult.blg || "", tool: bibResult.tool?.version });
        if (!bibResult.ok) return buildResult({ job, attempts, startedAt, ok: false, log: finalLog, failure: { kind: "bibliography", message: bibResult.error || bibResult.blg || "Biber failed", stage: "browser-biber" }, provenance: baseProvenance(engine, releaseId) });
        bibCache.set(identity, bibResult);
        bibliographyProvenance = "browser-biber";
        if (bibResult.tool?.version) bibliographyTools = { ...bibliographyTools, biber: bibResult.tool.version };
      } else {
        let reply2;
        try {
          reply2 = await call(target, "bibtex", { stem, eight: inspected.bibtex8 });
        } catch (error) {
          checkpoint();
          attempts.push({ stage: "browser", backend: "browser", ok: false, log: String(error?.message || error), reason: "bibliography" });
          return buildResult({
            job,
            attempts,
            startedAt,
            ok: false,
            log: finalLog,
            failure: { kind: classifyWorkerError(error, "bibliography"), message: String(error?.message || error), stage: "bibtex" },
            provenance: baseProvenance(engine, releaseId),
          });
        }
        checkpoint();
        bibResult = {
          ok: reply2.status === 0 || reply2.status === 1,
          bbl: toBytes(reply2.bbl),
          blg: reply2.blg || "",
          exit: reply2.status,
          tool: { name: inspected.bibtex8 ? "bibtex8" : "bibtex", version: "browser", backend: "browser" },
        };
        bibCache.set(identity, bibResult);
        bibliographyProvenance = "bibtex";
        attempts.push({ stage: "browser", backend: "browser", ok: bibResult.ok, log: bibResult.blg, tool: bibResult.tool.name });
      }

      if (bibResult?.bbl) {
        const previous = toBytes(outputs[`${stem}.bbl`]);
        bibliographyChanged = !previous || previous.length !== bibResult.bbl.length || previous.some((byte, i) => byte !== bibResult.bbl[i]);
        outputs[`${stem}.bbl`] = bibResult.bbl;
        try {
          await call(target, "write", { path: `${stem}.bbl`, bytes: bibResult.bbl.buffer || bibResult.bbl });
        } catch {
          /* the next tex pass will simply see the same undefined citations */
        }
        checkpoint();
      }
    }

    lastStaged = { project: currentProject, inputs, bibIdentity: bibliographyProvenance ? job.snapshot : lastStaged?.bibIdentity, generated: pickGenerated(outputs) };

    const needsRerun = (bibliographyChanged || inspected.biber || inspected.bibtex || inspected.bibtex8 || logMod.rerun(finalLog)) && passes < MAX_PASSES;
    if (!needsRerun) break;
  }

  attempts.push({ stage: "browser", backend: "browser", ok: true, log: finalLog });
  const diagnostics = logMod.parse(finalLog, { main: tree.main, paths: treePaths(tree) });
  return buildResult({
    job,
    attempts,
    startedAt,
    ok: true,
    pdf: lastPdf,
    synctex: lastSynctex,
    log: finalLog,
    diagnostics,
    failure: null,
    provenance: { backend: "browser", bibliography: bibliographyProvenance, engine, release: releaseId, tools: bibliographyTools },
  });
}

/// What the status says once nothing is running: the last compile's verdict,
/// or an idle store when no compile has finished in this session.
function settled(result) {
  if (!result) return { phase: "idle", message: "" };
  return {
    phase: result.ok ? "ready" : "failed",
    message: result.ok ? "Current preview ready" : "Compilation failed; previous preview shown",
  };
}

// --- The queue ---------------------------------------------------------------

function pump() {
  if (running || !queued) return;
  const { tree, waiting, failing } = queued;
  queued = null;
  running = true;
  jobGeneration += 1;
  const mine = jobGeneration;
  const token = { cancelled: false, abort: new AbortController() };
  activeToken = token;
  activeCallbacks = { waiting, failing };
  const startedAt = nowImpl();

  runCompile({ tree, jobGeneration: mine, token, startedAt })
    .then((result) => {
      if (token.cancelled) return;
      if (result.job.generation >= newestResolvedGeneration) {
        newestResolvedGeneration = result.job.generation;
        statusStore.set({
          lastResult: result,
          ...settled(result),
          backend: result.provenance?.backend || null,
          progress: null,
        });
      }
      for (const resolve of waiting) resolve(result);
    })
    .catch((error) => {
      if (token.cancelled) return;
      for (const reject of failing) reject(error);
    })
    .finally(() => {
      if (activeToken === token) {
        running = false;
        activeToken = null;
        activeCallbacks = null;
      }
      pump();
    });
}

/// Compiles a tree. `manual` marks a request the reader is starting
/// immediately rather than after its debounce; the queue behaves the same
/// either way; the reader is the one that decides when to call this.
export function compile(tree, { manual = false } = {}) {
  if (!currentProject) return Promise.reject(new Error("no LaTeX project configured"));
  return new Promise((resolve, reject) => {
    if (queued) {
      const stale = queued;
      queued = { tree, manual: manual || stale.manual, waiting: [...stale.waiting, resolve], failing: [...stale.failing, reject] };
    } else {
      queued = { tree, manual, waiting: [resolve], failing: [reject] };
    }
    pump();
  });
}

/// Discards the running and queued jobs. Both reject with
/// `error.name === "Superseded"`; the run already in flight keeps executing
/// in the background (a worker command cannot be un-awaited) but its result
/// is discarded by the same token every checkpoint above already checks.
export function cancel() {
  const error = supersededError();
  const discarded = Boolean(queued) || Boolean(activeToken && !activeToken.cancelled);
  if (queued) {
    for (const reject of queued.failing) reject(error);
    queued = null;
  }
  if (activeToken && !activeToken.cancelled) {
    activeToken.cancelled = true;
    activeToken.abort.abort();
    if (activeCallbacks) for (const reject of activeCallbacks.failing) reject(error);
    activeCallbacks = null;
    running = false;
    activeToken = null;
  }
  // A cancelled run is the one thing that never reaches the resolution above,
  // and the phase it set on its way in -- "loading", "compiling",
  // "browser-biber" -- is what the preview pane reads as "still compiling".
  // Leaving it there is what made that indicator outlive every compile in the
  // session. The status goes back to what the last finished compile said, or
  // to idle when there has not been one.
  if (discarded) {
    const last = statusStore.get().lastResult;
    statusStore.set({ ...settled(last), progress: null });
  }
  pump();
}

export function status() {
  return statusStore.get();
}

export function subscribe(listener) {
  return statusStore.subscribe(listener);
}

export const resources = {
  async size() {
    const module = await getResources();
    return module ? module.size() : 0;
  },
  async clear() {
    const module = await getResources();
    if (module) return module.clear();
  },
  async readiness(release, keys) {
    const module = await getResources();
    return module ? module.readiness(release, keys) : { ready: false, missing: keys || [] };
  },
  async persist() {
    const module = await getResources();
    if (module) return module.persist();
  },
};

/// Test-only injection. Not part of the public contract --
/// `latex-controller.mjs` is the only caller.
export const _testing = {
  inject({ worker: WorkerOverride, biber: browserBiberModule, resources: resourcesModule, fetch: fetchOverride, now } = {}) {
    if (WorkerOverride !== undefined) WorkerClass = WorkerOverride;
    if (browserBiberModule !== undefined) biberOverride = browserBiberModule;
    if (resourcesModule !== undefined) resourcesOverride = resourcesModule;
    if (fetchOverride !== undefined) {
      fetchImpl = fetchOverride;
      // A new fetch means a check is simulating a different deployment;
      // `manifest.json` is cached for the module's whole lifetime otherwise,
      // which would leak one scenario's manifest into the next.
      manifest = null;
      manifestPromise = null;
    }
    if (now !== undefined) nowImpl = now;
  },
  reset() {
    WorkerClass = typeof Worker !== "undefined" ? Worker : null;
    biberOverride = undefined;
    biberModule?.cancel();
    biberModule = undefined;
    resourcesOverride = undefined;
    fetchImpl = (...args) => fetch(...args);
    nowImpl = () => Date.now();
    manifest = null;
    manifestPromise = null;
  },
};
