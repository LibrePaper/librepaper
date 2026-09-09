// LaTeX, compiled in this browser, with local and Biber-VM fallbacks.
//
// This module is the controller docs/specs/latex-compiler.md describes: it owns exactly
// one module worker running the browser engine, speaks the section 2.4 protocol to it,
// and decides -- through `latex/route.js`'s pure state machine -- when a
// Biber request or a browser failure should instead go to the author's local
// LibrePaper app or, failing that, to a Biber-only virtual machine in the
// browser. `latex/jobs.js` gives every compile its identity, `latex/
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
//   - The VM is a last resort for bibliography work only. It is imported
//     with a dynamic `import()` inside the one function that can reach it,
//     so that a document that never needs Biber never causes so much as a
//     module fetch for it, let alone a download of its guest image.
//
// `latex/local.js`, `latex/vm.js` and `latex/worker.js` belong to the
// packages building the engine adapter and the local/VM bridge clients
// alongside this one. Every reference to the first two goes through a
// dynamic `import()`, and the worker through a constructor tests can
// replace, precisely so this file loaded and its own checks ran even before
// those modules existed -- and it keeps working unchanged now that they do,
// because this file was written against the interfaces they promised, not
// their earlier absence. `_testing.inject` is how `latex-controller.mjs`
// supplies fakes for all three; see its doc comment below. `latex/engine.js`
// is different: it is a small, dependency-free module, so it is imported
// statically like any ordinary dependency.

import * as jobsMod from "./latex/jobs.js";
import * as bibliography from "./latex/bibliography.js";
import * as routeMod from "./latex/route.js";

// Every decision is traced with its event, so a routing question is
// answerable from the console rather than from a rebuild.
const route = {
  ...routeMod,
  decide(event, state) {
    const decision = routeMod.decide(event, state);
    trace("route", event.type, "->", decision.action, decision.failure ? decision.failure.kind : "", event.message ? JSON.stringify(String(event.message).slice(0, 200)) : "", event.vmSupported === undefined ? "" : `vm=${event.vmSupported}`, event.validBcf === undefined ? "" : `bcf=${event.validBcf}`);
    return decision;
  },
};
import * as statusStore from "./latex/status.js";
import * as logMod from "./latex/log.js";
import * as engineMod from "./latex/engine.js";
import { snapshotDigest } from "./tree-digest.js";

export const DEBOUNCE = 1500;
export const DEFAULT_BASE = "/latex/";

/// The whole job's time budget and the bounded pass count SPEC "Browser
/// compilation controller" asks for. Both are exceeded as a reported
/// `failure.kind: "timeout"`, eligible for the same native fallback as any
/// other browser failure.
export const DEADLINE_MS = 240_000;
export const MAX_PASSES = 8;

let base = DEFAULT_BASE;

// Swappable seams. Production leaves every one of these at its default; only
// `_testing.inject` (used by `latex-controller.mjs`) ever changes them, which
// is what lets this file's own logic run under Node with no worker, no
// network and no real local app or VM anywhere in reach.
let WorkerClass = typeof Worker !== "undefined" ? Worker : null;
let localOverride; // undefined = try the real module; anything else, including null, is used as-is
let vmOverride;
let biberOverride;
let biberModule;
let resourcesOverride;
let fetchImpl = (...args) => fetch(...args);
let nowImpl = () => Date.now();

let manifest = null;
let manifestPromise = null;

let currentProject = null;
let currentSettings = { engine: "auto", release: null };

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

// Routing and bibliography state persist for the whole session (SPEC:
// "keep that project on the native route for the current editing session";
// "Reuse a warm VM and valid bibliography results so prose edits incur no
// [re]execution"). `bibCache` is keyed by `bibliography.identity()`'s hash,
// so a citation, database or style change simply misses it rather than
// needing an explicit invalidation path.
let routeState = route.initialState({});
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
  const error = new Error("Superseded");
  error.name = "Superseded";
  return error;
}

/// Points this module at a mirror. Called once, by whatever knows the
/// deployment's `--latex`; the tests call it with a local one.
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

let deploymentConfig = null;
let deploymentConfigPromise = null;

/// `/api/config`'s `biberVm` field: the browser bibliography VM's own
/// descriptor URL and the sha256 `vm.js` verifies it against, or `null` when
/// this deployment offers none (`--biber-vm` was not passed). The VM is
/// LibrePaper's own artefact, hosted separately from the LaTeX mirror, so it
/// is no longer named by `release.vm` -- fetched at most once per page load,
/// and only from the one path that ever reaches for it (`runBibliography`'s
/// "try-vm" branch), so a document that never needs Biber never causes this
/// request either.
async function loadBiberVm() {
  if (deploymentConfig) return deploymentConfig.biberVm ?? null;
  if (!deploymentConfigPromise) {
    deploymentConfigPromise = fetchImpl("/api/config").then(async (response) => {
      if (!response.ok) throw new Error(`/api/config fetch failed: ${response.status}`);
      deploymentConfig = await response.json();
      return deploymentConfig;
    });
  }
  try {
    const config = await deploymentConfigPromise;
    return config.biberVm ?? null;
  } finally {
    if (!deploymentConfig) deploymentConfigPromise = null;
  }
}

/// Called by the reader when a document opens. Resets the queue, the
/// session route and every per-project cache when the project itself
/// changes; a settings-only reconfigure of the same project is `setSettings`.
export function configure({ project, settings: nextSettings, mayCompile = true } = {}) {
  const changedProject = project !== currentProject;
  currentProject = project;
  currentSettings = { engine: "auto", release: null, ...(nextSettings || {}) };
  if (changedProject) {
    cancel();
    jobGeneration = 0;
    newestResolvedGeneration = -1;
    routeState = route.initialState({});
    bibCache.clear();
    lastStaged = null;
  }
  statusStore.set({
    phase: "idle",
    engine: currentSettings.engine === "auto" ? null : currentSettings.engine,
    release: currentSettings.release,
    route: routeState.route,
  });
  // The local bridge scopes its pairing to (origin, project), so it learns
  // both the moment the reader names the project; a token issued to one
  // document never rides along to another.
  void getLocal().then((local) => {
    local?.configure?.({ project, origin: typeof location !== "undefined" ? location.origin : "" });
  });
  void mayCompile; // reserved for the reader's own read-only gating, not this module's
}

export function settings() {
  return currentSettings;
}

/// An engine/release change. SPEC: a compile settings change must not let an
/// artifact identified only by unchanged source text be reused across it, so
/// this clears the bibliography cache and starts a fresh routing session
/// (native/VM attempt budgets included) rather than trying to reconcile them.
export function setSettings(next) {
  currentSettings = { ...currentSettings, ...next };
  routeState = route.initialState({});
  bibCache.clear();
  lastStaged = null;
  configuredRelease = null; // the next compile must (re)configure the worker for the new release
  cancel();
  statusStore.set({
    route: "browser",
    engine: currentSettings.engine === "auto" ? null : currentSettings.engine,
    release: currentSettings.release,
  });
}

export async function releases() {
  const data = await loadManifest();
  return {
    default: data.default_release,
    current: currentSettings.release || data.default_release,
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
// the "must load before the file exists" risk the worker/local/vm/resources
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
        const error = new Error(`the compiler did not answer ${cmd} within its time budget`);
        error.name = "WorkerTimeout";
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
  // worker has no page to resolve a relative "/latex/" from: it gets the
  // absolute form, as the mirror check hands it one.
  await call(target, "configure", { base: absoluteBase(), release: releaseEntry, format: manifestFormat });
  configuredRelease = releaseEntry.id;
  return target;
}

function classifyWorkerError(error, fallbackKind) {
  // "Worker death -> reject the active job (failure.kind='init' result...)"
  // regardless of which command was in flight when it happened.
  return error?.name === "WorkerDied" ? "init" : fallbackKind;
}

// --- Backend seams (local LibrePaper, the Biber VM)
// ----------------------------

async function getLocal() {
  if (localOverride !== undefined) return localOverride;
  try {
    return await import("./latex/local.js");
  } catch {
    return null;
  }
}

async function getVm() {
  if (vmOverride !== undefined) return vmOverride;
  try {
    return await import("./latex/vm.js");
  } catch (error) {
    trace("vm import failed", String(error?.message || error));
    return null;
  }
}

async function getResources() {
  if (resourcesOverride !== undefined) return resourcesOverride;
  try {
    return await import("./latex/resources.js");
  } catch {
    return null;
  }
}

function classifyLocalError(error) {
  const name = error?.name;
  if (name === "ToolMissing") return "local-tool-missing";
  if (name === "Denied" || name === "Unauthorized") return "local-denied";
  return "local-unreachable";
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

function toBytes(value) {
  if (value == null) return null;
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  return null;
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

// --- Bibliography routing ---------------------------------------------------

/// Drives `route.js`'s state machine for one bibliography requirement, from
/// the initial `biber-needed` decision through every follow-up event a real
/// attempt's outcome produces, until it either has a `BiberResult` or a
/// terminal `stop`/`show-browser` failure. This is the one place `latex/
/// local.js` and `latex/vm.js` are ever reached from a Biber-needed path.
async function vmEligibility() {
  // Importing `latex/vm.js` at all is deferred to exactly this call, and
  // this call happens only from inside `runBibliography` -- itself reached
  // only once a browser pass has already produced a usable bcf/aux and
  // something (local unreachable, tool missing, incompatible) has made the
  // VM a candidate. An ordinary successful browser-only document never runs
  // this function at all, let alone this line (SPEC: "a successful
  // browser-only document must never download or start it").
  const vm = await getVm();
  if (!vm) {
    trace("vm module unavailable");
    return false;
  }
  try {
    const answer = vm.supported?.();
    trace("vm supported", JSON.stringify(answer));
    return Boolean(answer?.ok);
  } catch (error) {
    trace("vm supported threw", String(error?.message || error));
    return false;
  }
}

async function runBibliography({ request, attempts, engine, release, token }) {
  if (release?.engines?.biber) {
    statusStore.set({ phase: "browser-biber", message: "Updating bibliography in browser", backend: "browser" });
    try {
      const backend = biberOverride !== undefined ? biberOverride : (biberModule ||= await import("./latex/biber.js"));
      if (token.cancelled) throw supersededError();
      const outcome = await backend.runBiber(request, {
        base: absoluteBase(), release, signal: token.abort.signal,
        onProgress: progress => { if (!token.cancelled) statusStore.set({ progress }); },
      });
      if (token.cancelled) throw supersededError();
      attempts.push({ stage: "browser-biber", backend: "browser", ok: outcome.ok, log: outcome.blg || "", tool: outcome.tool?.version });
      if (!outcome.ok) return { ok: false, failure: { kind: "bibliography", message: outcome.error || outcome.blg || "Biber failed", stage: "browser-biber" } };
      return { ok: true, result: outcome };
    } catch (error) {
      if (token.cancelled || error?.name === "AbortError") throw supersededError();
      // A runtime/download failure can still use the existing local fallback.
      // A Biber input error above is terminal and is never rerun elsewhere.
      attempts.push({ stage: "browser-biber", backend: "browser", ok: false, reason: "init", log: String(error) });
    }
  }
  routeState = { ...routeState, snapshot: request.job.snapshot };
  let decision = route.decide(
    { type: "biber-needed", identity: request.identity, validBcf: Boolean(request.bcf), vmSupported: await vmEligibility() },
    routeState,
  );
  routeState = decision.state;

  for (;;) {
    if (decision.action === "try-local-biber") {
      statusStore.set({ phase: "checking-local", message: "Checking local LibrePaper", backend: null });
      const local = await getLocal();
      if (!local) {
        decision = route.decide(
          {
            type: "local-unreachable",
            identity: request.identity,
            validBcf: Boolean(request.bcf),
            onlyBibliography: true,
            vmSupported: await vmEligibility(),
          },
          routeState,
        );
        routeState = decision.state;
        continue;
      }
      statusStore.set({ phase: "local-biber", message: "Running local Biber", backend: "local" });
      let outcome;
      try {
        outcome = await local.runBiber(request, {});
      } catch (error) {
        decision = route.decide(
          {
            type: classifyLocalError(error),
            identity: request.identity,
            validBcf: Boolean(request.bcf),
            onlyBibliography: true,
            message: error?.message,
            vmSupported: await vmEligibility(),
          },
          routeState,
        );
        routeState = decision.state;
        continue;
      }
      if (outcome.incompatible) {
        let localTexAvailable = false;
        try {
          const caps = await local.capabilities({});
          localTexAvailable = Boolean(caps?.tools?.[engine]?.available);
        } catch {
          /* treated as no usable local TeX */
        }
        decision = route.decide(
          {
            type: "local-incompatible",
            identity: request.identity,
            validBcf: Boolean(request.bcf),
            localTexAvailable,
            message: outcome.error,
            vmSupported: await vmEligibility(),
          },
          routeState,
        );
        routeState = decision.state;
        continue;
      }
      if (!outcome.ok) {
        decision = route.decide({ type: "local-biber-failed", message: outcome.error }, routeState);
        routeState = decision.state;
        continue;
      }
      attempts.push({ stage: "local-biber", backend: "local", ok: true, log: outcome.blg || "", tool: outcome.tool?.version });
      return { ok: true, result: outcome };
    }

    if (decision.action === "try-vm") {
      statusStore.set({ phase: "vm-preparing", message: "Preparing browser bibliography support", backend: "vm", progress: null });
      const vm = await getVm();
      if (!vm) {
        decision = route.decide({ type: "vm-unavailable", message: "Browser bibliography support is unavailable" }, routeState);
        routeState = decision.state;
        continue;
      }
      let outcome;
      try {
        // The VM is LibrePaper's own artefact, hosted separately from the
        // LaTeX mirror -- `release.vm` no longer names it -- so where it is
        // comes from this deployment's own `/api/config` rather than the
        // manifest.
        const biberVm = await loadBiberVm();
        if (!biberVm) {
          decision = route.decide({ type: "vm-unavailable", message: "no bibliography VM is configured" }, routeState);
          routeState = decision.state;
          continue;
        }
        await vm.prepare(biberVm, (progress) => statusStore.set({ progress }));
        statusStore.set({ phase: "vm-biber", message: "Updating bibliography in browser", backend: "vm", progress: null });
        outcome = await vm.runBiber(request, {});
      } catch (error) {
        decision = route.decide({ type: "vm-unavailable", message: String(error?.message || error) }, routeState);
        routeState = decision.state;
        continue;
      }
      if (!outcome.ok) {
        decision = route.decide({ type: "vm-failed", message: outcome.error || "Biber failed" }, routeState);
        routeState = decision.state;
        continue;
      }
      attempts.push({ stage: "vm-biber", backend: "vm", ok: true, log: outcome.blg || "", tool: outcome.tool?.version });
      return { ok: true, result: outcome };
    }

    // "stop" or "show-browser": terminal. Both leave the browser's own last
    // pass as the thing to present; the difference is only in whether the
    // controller keeps that pass's PDF (show-browser) or has nothing to show
    // at all because browser TeX itself never produced one (stop, reached
    // only from paths above that cannot occur once a bcf/aux already exists
    // -- see the callers).
    return { ok: false, failure: decision.failure, showBrowser: decision.action === "show-browser" };
  }
}

// --- Native and browser-failure handling ------------------------------------

async function localTexAvailableFor(engine) {
  const local = await getLocal();
  if (!local) return { local: null, available: false };
  try {
    const caps = await local.capabilities({});
    return { local, available: Boolean(caps?.tools?.[engine]?.available) };
  } catch {
    return { local, available: false };
  }
}

async function runNative({ job, tree, engine, releaseId, attempts, startedAt }) {
  const local = await getLocal();
  if (!local) {
    return buildResult({
      job,
      attempts,
      startedAt,
      ok: false,
      failure: { kind: "local-unavailable", message: "Local LibrePaper is unavailable", stage: "native" },
      provenance: baseProvenance(engine, releaseId),
    });
  }
  statusStore.set({ phase: "native", message: "Compiling locally", backend: "local", progress: null });
  let result;
  try {
    result = await local.runTex({ job, tree, engine, main: tree.main }, {});
  } catch (error) {
    routeState = route.decide({ type: "native-failed", message: String(error?.message || error) }, routeState).state;
    attempts.push({ stage: "native", backend: "local", ok: false, log: String(error?.message || error) });
    return buildResult({
      job,
      attempts,
      startedAt,
      ok: false,
      failure: { kind: "native", message: String(error?.message || error), stage: "native" },
      provenance: baseProvenance(engine, releaseId),
    });
  }
  if (!result.ok) {
    routeState = route.decide({ type: "native-failed", message: result.error || "The local build failed" }, routeState).state;
    attempts.push({ stage: "native", backend: "local", ok: false, log: result.log || "" });
    return buildResult({
      job,
      attempts,
      startedAt,
      ok: false,
      log: result.log,
      diagnostics: result.diagnostics,
      failure: { kind: "native", message: result.error || "The local build failed", stage: "native" },
      provenance: result.provenance || baseProvenance(engine, releaseId),
    });
  }
  routeState = route.decide({ type: "native-ok" }, routeState).state;
  statusStore.set({ route: "native" });
  attempts.push({ stage: "native", backend: "local", ok: true, log: result.log || "" });
  return buildResult({
    job,
    attempts,
    startedAt,
    ok: true,
    pdf: result.pdf,
    synctex: result.synctex,
    log: result.log,
    diagnostics: result.diagnostics,
    failure: null,
    provenance: result.provenance || { backend: "local", bibliography: null, engine, release: null, tools: {} },
  });
}

/// SPEC-latex.md "Precise failure messages": when the bundle index itself
/// says a `.sty`/`.cls` a document asked for is not in the browser mirror
/// (`worker.js`'s `tex()` reads this off the resolver evidence, see
/// driver.js), name it instead of the generic "the document failed to
/// compile", and show the same one-line pairing instruction the rest of the
/// interface already uses for "get the local app involved" -- see
/// `latex/local.js`'s `pairingInstruction()`. `null` when nothing was
/// reported absent, so the caller's ordinary message stands.
async function mirrorAbsentMessage(mirrorAbsent) {
  const names = [...new Set(Array.isArray(mirrorAbsent) ? mirrorAbsent : [])];
  if (!names.length) return null;
  const local = await getLocal();
  const instruction = local?.pairingInstruction
    ? local.pairingInstruction()
    : "Run `librepaper local start` on this computer and enter the pairing code it prints.";
  const named = names.length === 1 ? `"${names[0]}"` : `"${names[0]}" and ${names.length - 1} more`;
  return `Package ${named} is not available in the browser mirror. ${instruction}`;
}

async function handleBrowserFailure({ job, tree, engine, releaseId, attempts, startedAt, kind, message }) {
  routeState = { ...routeState, snapshot: job.snapshot };
  // The failure's own words are the reason; the log is the engine's, from
  // the attempt that just failed, and the diagnostics are read out of it so
  // a missing package or an undefined control sequence lands in the gutter
  // rather than behind a generic sentence.
  const lastLog = [...attempts].reverse().find((one) => one.stage === "browser")?.log || "";
  const { available: localUsable } = await localTexAvailableFor(engine);
  const decision = route.decide({ type: "browser-failed", kind, message, localUsable }, routeState);
  routeState = decision.state;
  if (decision.action === "try-native") {
    return runNative({ job, tree, engine, releaseId, attempts, startedAt });
  }
  return buildResult({
    job,
    attempts,
    startedAt,
    ok: false,
    log: lastLog,
    diagnostics: lastLog ? logMod.parse(lastLog, { main: tree.main, paths: treePaths(tree) }) : [],
    failure: decision.failure || { kind, message, stage: "browser" },
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
      release: currentSettings.release || "unknown",
    });
    return buildResult({
      job,
      attempts: [],
      startedAt,
      ok: false,
      failure: { kind: "resources", message: String(error?.message || error), stage: "browser" },
      provenance: baseProvenance(engine, currentSettings.release || null),
    });
  }
  checkpoint();

  const releaseId = currentSettings.release || manifestData.default_release;
  const inputs = await snapshotDigest(tree);
  const job = await jobsMod.makeJob({ project: currentProject, generation: generationAtStart, tree, inputs, engine, release: releaseId });
  checkpoint();

  const attempts = [];
  routeState = { ...routeState, snapshot: job.snapshot };

  const releaseEntry = manifestData.releases?.[releaseId];
  if (!releaseEntry) {
    const retained = Object.keys(manifestData.releases || {}).join(", ") || "none retained";
    return buildResult({
      job,
      attempts,
      startedAt,
      ok: false,
      failure: {
        kind: "resources",
        message: releaseId
          ? `LaTeX release "${releaseId}" is not available in this deployment's mirror. Retained releases: ${retained}.`
          : "This deployment's LaTeX mirror has no default engine release. The operator must build and deploy the mirror from the wasm-latex repository (make mirror, then make push there), or point --latex at a working one.",
        stage: "browser",
      },
      provenance: baseProvenance(engine, releaseId),
    });
  }
  if (engine === "lualatex" && !releaseEntry.engines?.luatex) {
    return buildResult({
      job, attempts, startedAt, ok: false,
      failure: { kind: "resources", message: "LuaLaTeX is not available in this release", stage: "browser" },
      provenance: baseProvenance(engine, releaseId),
    });
  }
  // "Keep that project on the native route for the current editing session."
  if (routeState.route === "native") {
    return runNative({ job, tree, engine, releaseId, attempts, startedAt });
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
        bibliographyProvenance = kind === "biber" ? (bibResult.tool?.backend === "browser" ? "browser-biber" : bibResult.tool?.backend === "vm" ? "vm-biber" : "local-biber") : "bibtex";
        if (bibResult.tool?.version) bibliographyTools = { ...bibliographyTools, biber: bibResult.tool.version };
      } else if (useBiber) {
        const request = { job, stem, main: tree.main, bcf: controlBytes, files, identity };
        const routed = await runBibliography({ request, attempts, engine, release: releaseEntry, token });
        checkpoint();
        if (!routed.ok) {
          if (routed.showBrowser) {
            attempts.push({ stage: "browser", backend: "browser", ok: true, log: finalLog });
            return buildResult({
              job,
              attempts,
              startedAt,
              ok: true,
              pdf: lastPdf,
              synctex: lastSynctex,
              log: finalLog,
              diagnostics: logMod.parse(finalLog, { main: tree.main, paths: treePaths(tree) }),
              failure: routed.failure,
              provenance: { backend: "browser", bibliography: null, engine, release: releaseId, tools: {} },
            });
          }
          return buildResult({ job, attempts, startedAt, ok: false, log: finalLog, failure: routed.failure, provenance: baseProvenance(engine, releaseId) });
        }
        bibResult = routed.result;
        bibCache.set(identity, bibResult);
        bibliographyProvenance = bibResult.tool?.backend === "browser" ? "browser-biber" : bibResult.tool?.backend === "vm" ? "vm-biber" : "local-biber";
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
          phase: result.ok ? "ready" : "failed",
          message: result.ok ? "Current preview ready" : "Compilation failed; previous preview shown",
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
  routeState = route.decide({ type: "canceled" }, routeState).state;
  pump();
}

export function status() {
  return statusStore.get();
}

export function subscribe(listener) {
  return statusStore.subscribe(listener);
}

/// "Provide 'Try browser compilation' to reset the route."
export function tryBrowser() {
  routeState = route.decide({ type: "try-browser" }, routeState).state;
  statusStore.set({ route: "browser" });
}

export const local = {
  async status() {
    const module = await getLocal();
    return module ? module.status() : statusStore.get().local;
  },
  async connect(code) {
    const module = await getLocal();
    if (!module) throw new Error("the local bridge is not available");
    return module.connect(code);
  },
  async disconnect() {
    const module = await getLocal();
    if (module) return module.disconnect();
  },
  async retry() {
    const module = await getLocal();
    if (module) return module.retry();
  },
  async setAddress(url) {
    const module = await getLocal();
    if (module) return module.setAddress(url);
  },
  async address() {
    const module = await getLocal();
    return module ? module.address() : "";
  },
  async capabilities(opts) {
    const module = await getLocal();
    if (!module) throw new Error("the local bridge is not available");
    return module.capabilities(opts);
  },
  async rescan() {
    const module = await getLocal();
    if (module) return module.rescan();
  },
  async openApp() {
    const module = await getLocal();
    return module ? module.openApp() : "Run `librepaper local start` and follow the printed instructions.";
  },
};

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

/// Test-only injection. Every field is optional; passing `null` for `local`
/// or `vm` explicitly simulates "checked and unavailable" rather than
/// "not yet checked", which the real dynamic import cannot distinguish from
/// outside. Not part of the public contract -- `latex-controller.mjs` is the
/// only caller.
export const _testing = {
  inject({ worker: WorkerOverride, local: localModule, vm: vmModule, biber: browserBiberModule, resources: resourcesModule, fetch: fetchOverride, now } = {}) {
    if (WorkerOverride !== undefined) WorkerClass = WorkerOverride;
    if (localModule !== undefined) localOverride = localModule;
    if (vmModule !== undefined) vmOverride = vmModule;
    if (browserBiberModule !== undefined) biberOverride = browserBiberModule;
    if (resourcesModule !== undefined) resourcesOverride = resourcesModule;
    if (fetchOverride !== undefined) {
      fetchImpl = fetchOverride;
      // A new fetch means a check is simulating a different deployment;
      // `manifest.json` and `/api/config` are cached for the module's whole
      // lifetime otherwise (SPEC: fetched at most once per page load), which
      // would leak one scenario's manifest or `biberVm` into the next.
      manifest = null;
      manifestPromise = null;
      deploymentConfig = null;
      deploymentConfigPromise = null;
    }
    if (now !== undefined) nowImpl = now;
  },
  reset() {
    WorkerClass = typeof Worker !== "undefined" ? Worker : null;
    localOverride = undefined;
    vmOverride = undefined;
    biberOverride = undefined;
    biberModule?.cancel();
    biberModule = undefined;
    resourcesOverride = undefined;
    fetchImpl = (...args) => fetch(...args);
    nowImpl = () => Date.now();
    manifest = null;
    manifestPromise = null;
    deploymentConfig = null;
    deploymentConfigPromise = null;
  },
};
