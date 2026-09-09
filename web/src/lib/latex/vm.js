// The browser Biber VM, as a client of its worker.
//
// This is the last bibliography fallback (SPEC "Browser Biber VM"): a small
// v86 guest, booted from this deployment's own pinned image (`/api/config`'s
// `biberVm`, set by `--biber-vm`; the VM is LibrePaper's own artefact, hosted
// separately from the LaTeX mirror and no longer named by a release), that
// runs the real `biber` binary on a BCF a successful WasmTex pass already
// produced. It never runs TeX, never touches the network beyond fetching its
// own pinned image, and is only ever started when local LibrePaper is
// unavailable and bibliography work remains.
//
// `vm-worker.js` owns the emulator; this file owns *when* to talk to it --
// one job at a time, a bounded boot, a bounded job, idle teardown, and a
// poisoned guest that refuses further work until `retire()` is called again
// through a fresh `prepare()`. The protocol between the two is documented at
// the top of `vm-worker.js`.
//
// `vm-worker.js` is a classic (non-module) worker -- it loads v86's own
// `libv86.js` with `importScripts`, which module workers do not have -- so
// it cannot `import` this file's helpers. The pure ones (path validation,
// marker parsing, incompatibility detection) are duplicated there instead,
// each pointing back here in a comment, and are exported here so
// `web/checks/latex-vm.mjs` can check the one canonical copy.
//
// Every ambient dependency -- the dynamic `resources.js` import (package B1,
// built concurrently against the same interface), `Worker`, `fetch`,
// `navigator`, the wall clock and the idle/boot/job timers -- goes through
// `deps`, so the check can run this exact module under Node with a fake
// worker standing in for the emulator.

const BOOT_TIMEOUT_MS = 120 * 1000;
const JOB_TIMEOUT_MS = 600 * 1000;
const IDLE_TEARDOWN_MS = 5 * 60 * 1000;
const CANCEL_GRACE_MS = 5 * 1000;

function defaultDeps() {
  return {
    fetch: (...args) => globalThis.fetch(...args),
    Worker: typeof Worker !== "undefined" ? Worker : undefined,
    resources: null, // set to override the dynamic import in checks
    navigator: () => (typeof navigator !== "undefined" ? navigator : {}),
    hasWebAssembly: () => typeof WebAssembly !== "undefined",
    now: () => Date.now(),
    setTimeout: (...args) => setTimeout(...args),
    clearTimeout: (...args) => clearTimeout(...args),
  };
}

let deps = defaultDeps();

export const _testing = {
  inject(overrides) { Object.assign(deps, overrides); },
  reset() {
    deps = defaultDeps();
    workerState = "cold";
    activeWorker = null;
    preparedFor = null;
    preparing = null;
    runningJob = null;
    queued = null;
    idleTimer = null;
  },
};

// -------------------------------------------------------------- pure helpers
//
// Shared with `vm-worker.js` (which keeps its own literal copy -- see the
// file header there) and exercised directly by the check.

/// A relative, non-traversing guest path, the same shape `biber-bridge.js`
/// enforced for the CheerpX prototype: no leading slash, no backslash, no
/// empty/`.`/`..` segment, no control character.
export function validateRelativePath(path) {
  if (
    typeof path !== "string" || !path || path.startsWith("/") || path.includes("\\") ||
    path.split("/").some((part) => !part || part === "." || part === "..") ||
    /[\u0000-\u001f\u007f]/.test(path)
  ) {
    throw new TypeError(`Invalid relative guest path: ${String(path)}`);
  }
  return path;
}

/// Parses `LIBREPAPER_DONE_<id>:<code>` out of the serial transcript captured
/// since a command started, the same marker `tinytex-v86/worker.js` uses.
export function parseDoneMarker(output, id) {
  const found = output.match(new RegExp("LIBREPAPER_DONE_" + id + ":(\\d+)\\r?\\n"));
  return found ? Number(found[1]) : null;
}

/// A control-file version mismatch is Biber refusing a BCF written by a
/// different biblatex than the one it was built against -- the guest is
/// reachable and ran, but this browser release and this VM image disagree.
export function detectIncompatible(blg) {
  return /control file version/i.test(String(blg || ""));
}

// -------------------------------------------------------------- support check

const IOS_UA = /iP(hone|ad|od)/;

export function supported() {
  if (!deps.hasWebAssembly()) return { ok: false, reason: "This browser has no WebAssembly support." };
  if (!deps.Worker) return { ok: false, reason: "This browser has no Worker support." };
  const nav = deps.navigator() || {};
  if (IOS_UA.test(nav.userAgent || "")) {
    return { ok: false, reason: "iOS Safari cannot run the bibliography VM." };
  }
  if (nav.deviceMemory !== undefined && nav.deviceMemory < 2) {
    return { ok: false, reason: "This device does not have enough memory for the bibliography VM." };
  }
  return { ok: true, reason: null };
}

// -------------------------------------------------------------- boot state

let workerState = "cold"; // "cold"|"loading"|"ready"|"busy"|"failed"
let activeWorker = null;
let preparedFor = null; // the vm release id currently booted
let preparing = null; // in-flight prepare() promise
let idleTimer = null;

export function state() {
  if (runningJob) return "busy";
  return workerState;
}

async function loadResources() {
  if (deps.resources) return deps.resources;
  return import("./resources.js");
}

function unsupportedError(reason) {
  const error = new Error(reason);
  error.name = "VmUnsupported";
  return error;
}

function unavailableError(reason) {
  const error = new Error(reason);
  error.name = "VmUnavailable";
  return error;
}

/// `vmConfig` is `/api/config`'s `biberVm` field: `{url, sha256}`, the
/// descriptor's own location and the digest to verify it against. The VM is
/// LibrePaper's own artefact, hosted separately from the LaTeX mirror -- it
/// is no longer named by `release.vm` -- so this is the whole of what a
/// caller needs to hand over; see `latex.js`'s `loadBiberVm`.
export async function prepare(vmConfig, onProgress) {
  const check = supported();
  if (!check.ok) throw unsupportedError(check.reason);
  if (!vmConfig?.url || !vmConfig?.sha256) throw unavailableError("no bibliography VM is configured");
  if (workerState === "ready" && preparedFor === vmConfig.url) return;
  if (preparing) return preparing;
  workerState = "loading";
  preparing = boot(vmConfig, onProgress)
    .then(() => {
      workerState = "ready";
      preparedFor = vmConfig.url;
      preparing = null;
      scheduleIdleTeardown();
    })
    .catch((error) => {
      workerState = "failed";
      preparing = null;
      throw error;
    });
  return preparing;
}

async function boot(vmConfig, onProgress) {
  const resources = await loadResources();
  const descriptorUrl = typeof location !== "undefined" ? new URL(vmConfig.url, location.href).href : vmConfig.url;
  // `resources.fetchVerified`/`prefetch` (package B1) cache under a
  // release's own digest; the VM is hosted separately from the LaTeX
  // mirror now, so it gets its own namespace, keyed off its own sha256
  // rather than an engine release's.
  const vmRelease = { digest: vmConfig.sha256 };
  const vmJsonResponse = await resources.fetchVerified(vmRelease, descriptorUrl, {
    sha256: vmConfig.sha256, size: vmConfig.size,
  });
  const vmJson = await vmJsonResponse.json();
  // Every URL `vm.json` names -- `objects`, and each file's own `url` -- is
  // relative to the descriptor's own location, the one place this VM is
  // hosted; `new URL` resolves a bare name and a deeper relative path the
  // same way.
  const beside = (url) => new URL(url, descriptorUrl).href;
  const entries = Object.entries(vmJson.files || {}).map(([name, meta]) => ({ name, url: beside(meta.url), sha256: meta.sha256, size: meta.size }));
  await resources.prefetch(vmRelease, entries, (progress) => onProgress?.({ ...progress, scope: "bibliography support" }));
  const byName = Object.fromEntries(entries.map((entry) => [entry.name, entry.url]));

  const worker = new deps.Worker(workerUrl());
  activeWorker = worker;
  worker.onmessage = onWorkerMessage;
  worker.onerror = (event) => failWorker(String(event?.message || event));

  await new Promise((resolve, reject) => {
    const timeout = deps.setTimeout(() => reject(new Error("The bibliography VM did not boot in time")), BOOT_TIMEOUT_MS);
    const off = onBootSettle((ok, error) => {
      deps.clearTimeout(timeout);
      off();
      if (ok) resolve(); else reject(new Error(error || "The bibliography VM failed to boot"));
    });
    worker.postMessage({
      type: "boot",
      config: {
        libv86Url: byName["libv86.js"],
        wasmPath: byName["v86.wasm"],
        biosUrl: byName["seabios.bin"],
        vgaBiosUrl: byName["vgabios.bin"],
        bzimageUrl: byName["bzimage"],
        basefsUrl: byName["fs.json"],
        baseurl: beside(vmJson.objects),
        memoryBytes: (vmJson.memory_mb || 256) * 1024 * 1024,
        ready: vmJson.boot?.ready || "LIBREPAPER_VM_READY",
        failed: vmJson.boot?.failed || "LIBREPAPER_VM_FAILED",
        setup: vmJson.boot?.setup || null,
        exec: vmJson.boot?.exec || null,
        cmdline: vmJson.boot?.cmdline || null,
      },
    });
  });
}

function workerUrl() {
  return new URL("./vm-worker.js", import.meta.url);
}

// A tiny pub/sub so `boot()` can await exactly the next boot outcome without
// tangling with `onWorkerMessage`'s job-routing responsibilities below.
let bootListeners = new Set();
function onBootSettle(listener) {
  bootListeners.add(listener);
  return () => bootListeners.delete(listener);
}

function failWorker(reason) {
  workerState = "failed";
  for (const listener of [...bootListeners]) listener(false, reason);
  if (runningJob) settleRunning({ error: reason });
}

function onWorkerMessage(event) {
  const msg = event.data;
  if (msg.type === "status") {
    if (msg.status === "ready") {
      for (const listener of [...bootListeners]) listener(true, null);
    } else if (msg.status === "error") {
      for (const listener of [...bootListeners]) listener(false, msg.error);
    }
    return;
  }
  if (msg.type === "job-result" || msg.type === "job-error") {
    if (!runningJob || runningJob.id !== msg.id) return;
    settleRunning(msg);
    return;
  }
  if (msg.type === "poisoned") {
    // The guest did not answer Ctrl-C within its grace period; its state
    // cannot be trusted again, so this is a `retire()`, not a plain job
    // failure -- the next job needs a fresh `prepare()` and boot.
    retire();
    return;
  }
}

// -------------------------------------------------------------- jobs
//
// One job at a time. A second call while one is already queued (not yet
// started) supersedes the queued one -- the running job is left alone, per
// SPEC "Do not interrupt an in-flight successful job merely because a
// preferred backend appeared."

let runningJob = null; // { id, request, resolve, reject, timeout, signal, onAbort }
let queued = null; // { request, opts, resolve, reject }
let sequence = 0;

export function runBiber(request, opts = {}) {
  return new Promise((resolve, reject) => {
    if (queued) {
      const error = new Error("Superseded by a newer bibliography job");
      error.name = "Superseded";
      queued.reject(error);
    }
    queued = { request, opts, resolve, reject };
    pump();
  });
}

function pump() {
  if (runningJob || !queued) return;
  const job = queued;
  queued = null;
  clearIdleTeardown();
  startJob(job.request, job.opts).then(
    (result) => { job.resolve(result); pump(); },
    (error) => { job.reject(error); pump(); },
  );
}

async function startJob(request, { signal, onProgress } = {}) {
  if (state() === "failed" || workerState !== "ready") {
    throw unavailableError("The bibliography VM is not ready.");
  }
  const id = `job_${++sequence}`;
  const stem = request.stem;
  const files = [[`${stem}.bcf`, bytesOf(request.bcf)]];
  for (const [path, bytes] of Object.entries(request.files || {})) {
    validateRelativePath(path);
    files.push([path, bytesOf(bytes)]);
  }
  onProgress?.({ done: 0, total: 1, scope: "bibliography support" });

  return new Promise((resolve, reject) => {
    const timeout = deps.setTimeout(() => {
      cancelRunning("The bibliography VM job timed out.");
    }, JOB_TIMEOUT_MS);

    const onAbort = () => cancelRunning(null, true);
    signal?.addEventListener("abort", onAbort, { once: true });

    runningJob = { id, resolve, reject, timeout, signal, onAbort, canceling: false };

    activeWorker.postMessage({
      type: "job",
      id, stem,
      files: files.map(([path, bytes]) => ({ path, bytes: toArrayBuffer(bytes) })),
    });
  });
}

function cancelRunning(timeoutReason, superseded = false) {
  if (!runningJob || runningJob.canceling) return;
  runningJob.canceling = true;
  activeWorker?.postMessage({ type: "cancel", id: runningJob.id, graceMs: CANCEL_GRACE_MS });
  if (timeoutReason) {
    // A timeout is also a possible poisoning; the worker answers job-error
    // or poisoned either way, so nothing else to do here but wait for it.
  }
  if (superseded) {
    // The caller's own AbortSignal fired; the eventual worker reply still
    // resolves the promise, just with a Canceled error.
  }
}

function settleRunning(msg) {
  const job = runningJob;
  runningJob = null;
  deps.clearTimeout(job.timeout);
  job.signal?.removeEventListener("abort", job.onAbort);
  scheduleIdleTeardown();
  if (msg.error) {
    const error = new Error(msg.error);
    error.name = msg.name || (job.canceling ? "Canceled" : "VmFailed");
    job.reject(error);
    return;
  }
  job.resolve({
    ok: msg.exitCode === 0 && msg.bbl != null,
    bbl: msg.bbl ? new Uint8Array(msg.bbl) : null,
    blg: new TextDecoder().decode(msg.blg ? new Uint8Array(msg.blg) : new Uint8Array()),
    exit: msg.exitCode,
    tool: { name: "biber", version: msg.biberVersion || null, backend: "vm" },
    incompatible: !!msg.incompatible,
  });
}

function bytesOf(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (typeof value === "string") return new TextEncoder().encode(value);
  throw new TypeError("expected bytes");
}

function toArrayBuffer(bytes) {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}

// -------------------------------------------------------------- teardown

function clearIdleTeardown() {
  if (idleTimer != null) deps.clearTimeout(idleTimer);
  idleTimer = null;
}

function scheduleIdleTeardown() {
  clearIdleTeardown();
  idleTimer = deps.setTimeout(() => {
    if (!runningJob && !queued) retire();
  }, IDLE_TEARDOWN_MS);
}

export function retire() {
  clearIdleTeardown();
  try { activeWorker?.postMessage({ type: "retire" }); } catch { /* best effort */ }
  try { activeWorker?.terminate?.(); } catch { /* best effort */ }
  activeWorker = null;
  workerState = "cold";
  preparedFor = null;
  if (runningJob) {
    settleRunning({ error: "The bibliography VM was retired", name: "VmUnavailable" });
  }
}
