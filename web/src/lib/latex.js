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
import { snapshotDigest } from "./projection-digest.js";
import { maybeBytes as toBytes } from "./bytes.js";
import { named } from "./latex/errors.js";

export const DEBOUNCE = 1500;

/// The whole job's time budget and the bounded pass count SPEC "Browser
/// compilation controller" asks for. Both are exceeded as a reported
/// `failure.kind: "timeout"`.
export const DEADLINE_MS = 240_000;
export const MAX_PASSES = 8;

let base = "";

// Swappable seams. Production leaves every one of these at its default; only
// `_testing.inject` (used by `latex-controller.mjs`) ever changes them, which
// is what lets this file's own logic run under Node with no worker, no
// network and no real local app anywhere in reach.
let WorkerClass = typeof Worker !== "undefined" ? Worker : null;
let biberOverride;
let biberModule;
let bibliographyIdentityOverride;
let snapshotDigestOverride;
let resourcesOverride;
let fetchImpl = (input, init) => fetch(input, init);
let nowImpl = () => Date.now();
let deadlineMs = DEADLINE_MS;

let release = null;
let releasePromise = null;

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

/// Points this module at the pinned release directory (`<mirror>latex/<id>/`)
/// advertised by `/api/config`. The browser never picks a release.
export function at(url) {
  if (url && url !== base) {
    base = url.endsWith("/") ? url : url + "/";
    release = null;
    releasePromise = null;
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

async function loadRelease() {
  if (release) return release;
  if (!releasePromise) {
    const requestedBase = base;
    let pending;
    pending = fetchImpl(absoluteBase() + "release.json").then(async (response) => {
      if (!response.ok) throw new Error(`no LaTeX release at ${base} (${response.status})`);
      const data = await response.json();
      if (data.format !== 2) {
        throw new Error(`this LaTeX release is format ${data.format ?? "unknown"}, but this build only speaks format 2; rebuild the mirror with make mirror in wasm-latex`);
      }
      // A request abandoned by a timeout or mirror switch must not repopulate
      // the cache for the newer request.
      if (releasePromise === pending && base === requestedBase) release = data;
      return data;
    });
    releasePromise = pending;
  }
  const pending = releasePromise;
  try {
    return await pending;
  } finally {
    // A failed fetch must not poison the module: the next compile tries
    // again rather than repeating a network error forever from cache.
    if (!release && releasePromise === pending) releasePromise = null;
  }
}

function timeoutError(what = "the compile") {
  return named("WorkerTimeout", `${what} exceeded its time budget`);
}

function remaining(deadlineAt) {
  return Math.max(0, deadlineAt - nowImpl());
}

function withinDeadline(promise, deadlineAt, what, onTimeout, signal) {
  const work = Promise.resolve(promise);
  // When the budget is already gone, the caller still owns this operation;
  // consume its eventual rejection even though the job stops waiting now.
  work.catch(() => {});
  if (signal?.aborted) return Promise.reject(supersededError());
  const ms = remaining(deadlineAt);
  if (!ms) {
    onTimeout?.();
    return Promise.reject(timeoutError(what));
  }
  let timer;
  let abort;
  const races = [
    work,
    new Promise((_, reject) => { timer = setTimeout(() => { onTimeout?.(); reject(timeoutError(what)); }, ms); }),
  ];
  if (signal) races.push(new Promise((_, reject) => {
    abort = () => reject(supersededError());
    signal.addEventListener("abort", abort, { once: true });
  }));
  return Promise.race(races).finally(() => {
    clearTimeout(timer);
    if (abort) signal.removeEventListener("abort", abort);
  });
}

function deadlineCall(target, cmd, payload, deadlineAt) {
  const ms = remaining(deadlineAt);
  if (!ms) return Promise.reject(timeoutError(`worker ${cmd}`));
  return call(target, cmd, payload, { timeoutMs: ms });
}

async function makeJobWithinDeadline({ tree, generation, engine, release, deadlineAt, signal }) {
  if (signal?.aborted) throw supersededError();
  if (!remaining(deadlineAt)) throw timeoutError("document digest");
  const inputs = await withinDeadline((snapshotDigestOverride || snapshotDigest)(tree), deadlineAt, "document digest", undefined, signal);
  if (signal?.aborted) throw supersededError();
  if (!remaining(deadlineAt)) throw timeoutError("job identity");
  const job = await withinDeadline(
    jobsMod.makeJob({ project: currentProject, generation, tree, inputs, engine, release }),
    deadlineAt,
    "job identity",
    undefined,
    signal,
  );
  return { inputs, job };
}

/// Called by the reader when a document opens. Resets the queue, the
/// session route and every per-project cache when the project itself
/// changes; a settings-only reconfigure of the same project is `setSettings`.
/** @param {{project?: string|null, settings?: object}} [options] */
export function configure({ project, settings: nextSettings } = {}) {
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
}

export function settings() {
  return currentSettings;
}

function normalizeSettings(next = {}) {
  const backend = ["auto", "browser", "local"].includes(next.backend) ? next.backend : "auto";
  const tool = typeof next.tool === "string" && next.tool ? next.tool : "tex";
  const engine = ["auto", "pdflatex", "xelatex", "lualatex"].includes(next.engine) ? next.engine : "auto";
  return { engine, backend, tool, output: next.output || "html", options: next.options && typeof next.options === "object" ? { ...next.options } : {} };
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

async function ensureWorker(releaseEntry, deadlineAt) {
  if (worker && configuredRelease === releaseEntry.id) return worker;
  if (worker) retireWorker();
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
  // The worker receives the pinned release directory URL, so all release
  // paths resolve against the direct mirror rather than an app route.
  await deadlineCall(target, "configure", { base: absoluteBase(), release: releaseEntry }, deadlineAt);
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

/** @param {Record<string, string|Uint8Array|ArrayBuffer>} raw @returns {Record<string, string|Uint8Array>} */
function normalizeOutputs(raw) {
  /** @type {Record<string, string|Uint8Array>} */
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

function buildResult({ job, attempts, startedAt, ok, pdf = null, synctex = null, log = "", diagnostics = undefined, failure, provenance }) {
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
  const deadlineAt = startedAt + deadlineMs;
  const checkpoint = () => {
    if (token.cancelled) throw supersededError();
  };

  const engine = resolveEngine(tree, currentSettings);
  const timedOutBeforeJob = (releaseId) => buildResult({
    job: {
      id: `${currentProject}:${generationAtStart}`,
      project: currentProject,
      generation: generationAtStart,
      // No snapshot digest is available when its own deadline expired.
      snapshot: "",
      inputs: "",
      main: tree?.main ?? "",
      engine,
      release: releaseId,
    },
    attempts: [],
    startedAt,
    ok: false,
    failure: { kind: "timeout", message: "The compile exceeded its time budget.", stage: "browser" },
    provenance: baseProvenance(engine, releaseId === "unknown" ? null : releaseId),
  });

  let releaseEntry;
  try {
    const loadingRelease = loadRelease();
    const pendingRelease = releasePromise;
    releaseEntry = await withinDeadline(loadingRelease, deadlineAt, "LaTeX release load", () => {
      // A timed-out fetch/body read must not pin later queued jobs to the
      // same unresolved cache promise. The old request may finish harmlessly.
      if (!release && releasePromise === pendingRelease) releasePromise = null;
    }, token.abort.signal);
  } catch (error) {
    checkpoint();
    const releaseId = releaseEntry?.id || "unknown";
    if (error?.name === "WorkerTimeout" || nowImpl() >= deadlineAt) return timedOutBeforeJob(releaseId);
    let job;
    try {
      ({ job } = await makeJobWithinDeadline({ tree, generation: generationAtStart, engine, release: releaseId, deadlineAt, signal: token.abort.signal }));
      checkpoint();
    } catch (identityError) {
      checkpoint();
      if (identityError?.name === "WorkerTimeout" || nowImpl() >= deadlineAt) return timedOutBeforeJob(releaseId);
      throw identityError;
    }
    return buildResult({
      job,
      attempts: [],
      startedAt,
      ok: false,
      failure: { kind: "resources", message: String(error?.message || error), stage: "browser" },
      provenance: baseProvenance(engine, releaseEntry?.id || null),
    });
  }
  checkpoint();

  const releaseId = releaseEntry.id;
  let inputs;
  let job;
  try {
    ({ inputs, job } = await makeJobWithinDeadline({ tree, generation: generationAtStart, engine, release: releaseId, deadlineAt, signal: token.abort.signal }));
    checkpoint();
  } catch (error) {
    checkpoint();
    if (error?.name === "WorkerTimeout" || nowImpl() >= deadlineAt) return timedOutBeforeJob(releaseId);
    throw error;
  }

  const attempts = [];
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
    target = await ensureWorker(releaseEntry, deadlineAt);
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
      kind: error?.name === "WorkerTimeout" ? "timeout" : classifyWorkerError(error, "init"),
      message: String(error?.message || error),
    });
  }
  checkpoint();

  statusStore.set({ phase: "compiling", message: "Compiling in browser", backend: "browser", release: releaseId, engine });

  const generated = lastStaged && lastStaged.project === currentProject ? lastStaged.generated : {};
  try {
    await deadlineCall(target, "stage", { engine, tree, generated }, deadlineAt);
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
      kind: error?.name === "WorkerTimeout" ? "timeout" : classifyWorkerError(error, "resources"),
      message: String(error?.message || error),
    });
  }
  checkpoint();

  const stem = stemOf(tree.main);
  /** @type {Record<string, string|Uint8Array>} */
  let outputs = {};
  let finalLog = "";
  let lastPdf = null;
  let lastSynctex = null;
  let bibliographyProvenance = null;
  let bibliographyTools = {};
  let passes = 0;
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

    const beforeGenerated = pickGenerated(outputs);
    let reply;
    try {
      reply = await deadlineCall(target, "tex", { engine, main: tree.main }, deadlineAt);
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
      const missingMessage = await mirrorAbsentMessage(reply.mirrorAbsent);
      checkpoint();
      return handleBrowserFailure({
        job,
        tree,
        engine,
        releaseId,
        attempts,
        startedAt,
        signal: token.abort.signal,
        kind: "tex",
        message: missingMessage || "The document failed to compile.",
      });
    }

    const inspected = bibliography.inspect({ stem, outputs, log: finalLog, tree });

    let indexChanged = false;
    if (inspected.makeindex) {
      try {
        const idx = await deadlineCall(target, "makeindex", { stem }, deadlineAt);
        checkpoint();
        const ind = toBytes(idx.ind);
        if (!(idx.status === 0 || idx.status === 1) || !ind) {
          throw new Error(idx.ilg || "MakeIndex did not produce an index.");
        }
        const previous = toBytes(outputs[`${stem}.ind`]);
        indexChanged = !previous || previous.length !== ind.length || previous.some((byte, i) => byte !== ind[i]);
        outputs[`${stem}.ind`] = ind;
        await deadlineCall(target, "write", { path: `${stem}.ind`, bytes: ind.buffer || ind }, deadlineAt);
        checkpoint();
      } catch (error) {
        checkpoint();
        if (error?.name === "WorkerTimeout") {
          attempts.push({ stage: "makeindex", backend: "browser", ok: false, log: String(error.message), reason: "timeout" });
          return handleBrowserFailure({ job, tree, engine, releaseId, attempts, startedAt, signal: token.abort.signal, kind: "timeout", message: String(error.message) });
        }
        attempts.push({ stage: "makeindex", backend: "browser", ok: false, log: String(error?.message || error), reason: "index" });
        // TeX has already produced a usable PDF. Keep it visible and retain
        // the failed helper attempt as diagnostic provenance; a broken index
        // must not replace the whole document with a failure page.
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
      let identity;
      try {
        if (!remaining(deadlineAt)) throw timeoutError("bibliography identity");
        identity = await withinDeadline(
          (bibliographyIdentityOverride || bibliography.identity)({ kind, controlBytes, files, engine, release: releaseId, tool: kind }),
          deadlineAt,
          "bibliography identity",
          undefined,
          token.abort.signal,
        );
        checkpoint();
      } catch (error) {
        checkpoint();
        if (error?.name === "WorkerTimeout" || nowImpl() >= deadlineAt) {
          attempts.push({ stage: "browser-bibliography", backend: "browser", ok: false, log: String(error?.message || error), reason: "timeout" });
          return buildResult({
            job, attempts, startedAt, ok: false, log: finalLog,
            failure: { kind: "timeout", message: "The compile exceeded its time budget.", stage: "browser-bibliography" },
            provenance: baseProvenance(engine, releaseId),
          });
        }
        throw error;
      }

      let bibResult = bibCache.get(identity) || null;
      if (bibResult) {
        bibliographyProvenance = kind === "biber" ? "browser-biber" : "bibtex";
        if (bibResult.tool?.version) bibliographyTools = { ...bibliographyTools, biber: bibResult.tool.version };
      } else if (useBiber) {
        const request = { job, stem, main: tree.main, bcf: controlBytes, files, identity };
        statusStore.set({ phase: "browser-biber", message: "Updating bibliography in browser", backend: "browser" });
        const biberAbort = new AbortController();
        const relayAbort = () => biberAbort.abort();
        token.abort.signal.addEventListener("abort", relayAbort, { once: true });
        let biberTimedOut = false;
        try {
          bibResult = await withinDeadline((async () => {
            const backend = biberOverride !== undefined ? biberOverride : (biberModule ||= await import("./latex/biber.js"));
            if (token.cancelled || biberAbort.signal.aborted) throw supersededError();
            return backend.runBiber(request, {
              base: absoluteBase(), release: releaseEntry, signal: biberAbort.signal,
              // The Biber promise can keep running after this job times out.
              // Its late progress must not overwrite the status of a queued
              // compile that has since become current.
              onProgress: progress => {
                if (!token.cancelled && !biberAbort.signal.aborted) statusStore.set({ progress });
              },
            });
          })(), deadlineAt, "browser Biber", () => { biberTimedOut = true; biberAbort.abort(); }, token.abort.signal);
          checkpoint();
        } catch (error) {
          checkpoint();
          if (token.cancelled || error?.name === "Superseded") throw supersededError();
          if (biberTimedOut || error?.name === "WorkerTimeout" || nowImpl() >= deadlineAt) {
            return buildResult({ job, attempts, startedAt, ok: false, log: finalLog, failure: { kind: "timeout", message: "The compile exceeded its time budget.", stage: "browser-biber" }, provenance: baseProvenance(engine, releaseId) });
          }
          if (token.cancelled || error?.name === "AbortError") throw supersededError();
          bibResult = { ok: false, error: String(error?.message || error), blg: String(error), tool: { backend: "browser" } };
        } finally {
          token.abort.signal.removeEventListener("abort", relayAbort);
          if (!bibResult && !token.cancelled) biberAbort.abort();
        }
        attempts.push({ stage: "browser-biber", backend: "browser", ok: bibResult.ok, log: bibResult.blg || "", tool: bibResult.tool?.version });
        if (!bibResult.ok) return buildResult({ job, attempts, startedAt, ok: false, log: finalLog, failure: { kind: "bibliography", message: bibResult.error || bibResult.blg || "Biber failed", stage: "browser-biber" }, provenance: baseProvenance(engine, releaseId) });
        bibCache.set(identity, bibResult);
        bibliographyProvenance = "browser-biber";
        if (bibResult.tool?.version) bibliographyTools = { ...bibliographyTools, biber: bibResult.tool.version };
      } else {
        let reply2;
        try {
          reply2 = await deadlineCall(target, "bibtex", { stem, eight: inspected.bibtex8 }, deadlineAt);
        } catch (error) {
          checkpoint();
          attempts.push({ stage: "browser", backend: "browser", ok: false, log: String(error?.message || error), reason: "bibliography" });
          return buildResult({
            job,
            attempts,
            startedAt,
            ok: false,
            log: finalLog,
            failure: { kind: error?.name === "WorkerTimeout" ? "timeout" : classifyWorkerError(error, "bibliography"), message: String(error?.message || error), stage: "bibtex" },
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
          await deadlineCall(target, "write", { path: `${stem}.bbl`, bytes: bibResult.bbl.buffer || bibResult.bbl }, deadlineAt);
        } catch (error) {
          checkpoint();
          if (error?.name === "WorkerTimeout") return buildResult({ job, attempts, startedAt, ok: false, log: finalLog, failure: { kind: "timeout", message: error.message, stage: "write-bibliography" }, provenance: baseProvenance(engine, releaseId) });
          /* the next tex pass will simply see the same undefined citations */
        }
        checkpoint();
      }
    }

    lastStaged = { project: currentProject, inputs, bibIdentity: bibliographyProvenance ? job.snapshot : lastStaged?.bibIdentity, generated: pickGenerated(outputs) };

    const afterGenerated = pickGenerated(outputs);
    const generatedChanged = Object.keys({ ...beforeGenerated, ...afterGenerated }).some((path) => {
      const before = toBytes(beforeGenerated[path]);
      const after = toBytes(afterGenerated[path]);
      return !before || !after || before.length !== after.length || before.some((byte, index) => byte !== after[index]);
    });
    // Undefined-reference warnings alone are not progress. A typo should
    // retain its diagnostic without spending every pass budget forever.
    const explicitlyRequestsRerun = /Label\(s\) may have changed|Rerun to get cross-references right/i.test(finalLog);
    const needsRerun = (indexChanged || bibliographyChanged || explicitlyRequestsRerun
      || (logMod.rerun(finalLog) && generatedChanged)) && passes < MAX_PASSES;
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
    // A pending RPC still has its own deadline timer. Do not let it outlive
    // this job against a worker the replacement compile could reuse: retire
    // the worker now, which settles every canceled RPC and clears its timer.
    if (pendingCalls.size) retireWorker();
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
  /** @param {{ worker?: any, biber?: any, bibliographyIdentity?: (input: any) => Promise<string>, snapshotDigest?: (tree: any) => Promise<string>, resources?: any, fetch?: typeof fetch, now?: () => number, deadlineMs?: number }} [options] */
  inject({ worker: WorkerOverride, biber: browserBiberModule, bibliographyIdentity, snapshotDigest: snapshotDigestImpl, resources: resourcesModule, fetch: fetchOverride, now, deadlineMs: deadlineOverride } = {}) {
    if (WorkerOverride !== undefined) WorkerClass = WorkerOverride;
    if (browserBiberModule !== undefined) biberOverride = browserBiberModule;
    if (bibliographyIdentity !== undefined) bibliographyIdentityOverride = bibliographyIdentity;
    if (snapshotDigestImpl !== undefined) snapshotDigestOverride = snapshotDigestImpl;
    if (resourcesModule !== undefined) resourcesOverride = resourcesModule;
    if (fetchOverride !== undefined) {
      fetchImpl = fetchOverride;
      // A new fetch means a check is simulating a different deployment;
      // `release.json` is cached for the module's whole lifetime otherwise,
      // which would leak one scenario's release into the next.
      release = null;
      releasePromise = null;
    }
    if (now !== undefined) nowImpl = now;
    if (deadlineOverride !== undefined) deadlineMs = deadlineOverride;
  },
  reset() {
    WorkerClass = typeof Worker !== "undefined" ? Worker : null;
    biberOverride = undefined;
    bibliographyIdentityOverride = undefined;
    snapshotDigestOverride = undefined;
    biberModule?.cancel();
    biberModule = undefined;
    resourcesOverride = undefined;
    fetchImpl = (input, init) => fetch(input, init);
    nowImpl = () => Date.now();
    deadlineMs = DEADLINE_MS;
    release = null;
    releasePromise = null;
  },
};
