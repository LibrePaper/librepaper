// The bibliography VM client, against a fake worker standing in for
// `vm-worker.js`.
//
// `vm.js` never touches `Worker`, `fetch`, `import("./resources.js")` or the
// wall clock directly -- everything goes through its `_testing.inject`-able
// `deps` -- so this check drives the exact production module under Node
// against a `FakeWorker` that speaks the documented protocol from
// `vm-worker.js`'s header, without booting an actual v86 guest. It also
// checks the pure helpers (`validateRelativePath`, `parseDoneMarker`,
// `detectIncompatible`) that `vm-worker.js` keeps its own literal copy of.
//
// `vm-worker.js` is a classic worker -- it needs `importScripts` for v86s
// `libv86.js`, which module workers do not have -- so it genuinely cannot
// import those helpers from `vm.js`, and the copies have to stay. What was
// missing was any way to notice them drifting apart: the two copies were kept
// in step by a comment asking the next editor to remember, and one of them is
// the guest-path traversal guard, where drift is a sandbox bug rather than an
// untidiness. `testWorkerCopiesAgree` below lifts both functions out of the
// workers source and runs them against the same tables as the canonical ones.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import * as vm from "../src/lib/latex/vm.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`latex-vm: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}
async function rejects(promise, name, what) {
  try {
    await promise;
    check(what, false, "resolved instead of rejecting");
    return null;
  } catch (error) {
    check(what, error?.name === name, `got name ${error?.name}, wanted ${name} (${error?.message})`);
    return error;
  }
}

/* --------------------------------------------------------- pure helpers */

// The tables live at module scope rather than inline because
// `testWorkerCopiesAgree` runs `vm-worker.js`'s own copies of these helpers
// against exactly the same inputs. A case added here is therefore a case the
// worker's copy has to agree on too.
const VALID_PATHS = ["refs.bib", "assets/img/fig.pdf"];
const INVALID_PATHS = ["/etc/passwd", "..\\x", "a/../b", "a/./b", "a//b", "", "a\x00b", "a\x7fb"];
const BLG_SAMPLES = [
  ["ERROR - Found biblatex control file version 3.2, expected 3.4", true],
  ["INFO - This is Biber 2.21\nINFO - Reading main.bcf\n", false],
  [undefined, false],
];

function testValidateRelativePath() {
  for (const good of VALID_PATHS) {
    check(`a relative path is valid: ${JSON.stringify(good)}`, vm.validateRelativePath(good) === good);
  }
  for (const bad of INVALID_PATHS) {
    let threw = false;
    try { vm.validateRelativePath(bad); } catch { threw = true; }
    check(`rejects ${JSON.stringify(bad)}`, threw);
  }
}

function testParseDoneMarker() {
  check("finds the marker and exit code", vm.parseDoneMarker("blah\nLIBREPAPER_DONE_job1_biber:0\r\n", "job1_biber") === 0);
  check("finds a nonzero exit code", vm.parseDoneMarker("LIBREPAPER_DONE_x:2\n", "x") === 2);
  check("returns null when absent", vm.parseDoneMarker("no marker here", "x") === null);
}

function testDetectIncompatible() {
  for (const [blg, wanted] of BLG_SAMPLES) {
    check(`detectIncompatible(${JSON.stringify(blg)}) is ${wanted}`, vm.detectIncompatible(blg) === wanted);
  }
}

/// One of `vm-worker.js`'s literal copies, lifted out of its source text.
///
/// The worker can be neither imported here nor made to import `vm.js`, so
/// reading the function out and evaluating it is the only way to exercise the
/// code that will really run alongside the emulator. Brace counting is sound
/// for these two helpers -- neither contains a string, comment or regex with
/// an unbalanced brace -- and the check fails loudly if a rename ever moves
/// them, which is itself worth knowing.
function workerCopy(name) {
  const source = readFileSync(fileURLToPath(new URL("../src/lib/latex/vm-worker.js", import.meta.url)), "utf8");
  const start = source.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`vm-worker.js no longer defines ${name}`);
  let depth = 0;
  let end = -1;
  for (let at = source.indexOf("{", start); at < source.length; at++) {
    if (source[at] === "{") depth += 1;
    else if (source[at] === "}" && (depth -= 1) === 0) { end = at + 1; break; }
  }
  if (end < 0) throw new Error(`could not read ${name} out of vm-worker.js`);
  return new Function(`return (${source.slice(start, end)});`)();
}

// The drift guard the "keep the two in sync" comments were asking for. One of
// these copies is the guest-path traversal check, so a copy that has quietly
// fallen behind is a sandbox hole, not a tidiness problem.
function testWorkerCopiesAgree() {
  const validate = workerCopy("validateRelativePath");
  const detect = workerCopy("detectIncompatible");
  const verdict = (fn, path) => { try { return `ok:${fn(path)}`; } catch { return "threw"; } };

  for (const path of [...VALID_PATHS, ...INVALID_PATHS]) {
    const canonical = verdict(vm.validateRelativePath, path);
    const copy = verdict(validate, path);
    check(
      `vm-worker.js validateRelativePath agrees on ${JSON.stringify(path)}`,
      canonical === copy,
      `vm.js says ${canonical}, vm-worker.js says ${copy}`,
    );
  }
  for (const [blg] of BLG_SAMPLES) {
    check(
      `vm-worker.js detectIncompatible agrees on ${JSON.stringify(blg)}`,
      detect(blg) === vm.detectIncompatible(blg),
      `vm.js says ${vm.detectIncompatible(blg)}, vm-worker.js says ${detect(blg)}`,
    );
  }
}

/* --------------------------------------------------------- supported() */

function testSupported() {
  vm._testing.reset();
  vm._testing.inject({ hasWebAssembly: () => true, Worker: function () {}, navigator: () => ({ userAgent: "Mozilla/5.0 (X11; Linux x86_64)" }) });
  check("supported() passes with a plain desktop UA", vm.supported().ok === true);

  vm._testing.inject({ hasWebAssembly: () => false });
  check("no WebAssembly is unsupported", vm.supported().ok === false);
  vm._testing.inject({ hasWebAssembly: () => true });

  vm._testing.inject({ Worker: undefined });
  check("no Worker is unsupported", vm.supported().ok === false);
  vm._testing.inject({ Worker: function () {} });

  vm._testing.inject({ navigator: () => ({ userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)" }) });
  check("iOS Safari is unsupported", vm.supported().ok === false && /iOS/.test(vm.supported().reason));

  vm._testing.inject({ navigator: () => ({ userAgent: "Mozilla/5.0", deviceMemory: 1 }) });
  check("deviceMemory below 2 is unsupported", vm.supported().ok === false);

  vm._testing.inject({ navigator: () => ({ userAgent: "Mozilla/5.0", deviceMemory: 4 }) });
  check("deviceMemory at or above 2 is supported", vm.supported().ok === true);

  vm._testing.inject({ navigator: () => ({ userAgent: "Mozilla/5.0" }) });
  check("an unreported deviceMemory does not block", vm.supported().ok === true);
}

/* --------------------------------------------------------- fake worker & resources */

class FakeWorker {
  constructor(url) {
    this.url = url;
    this.onmessage = null;
    this.onerror = null;
    this.posted = [];
    this.terminated = false;
    FakeWorker.current = this;
  }
  postMessage(msg) {
    this.posted.push(msg);
    FakeWorker.handler?.(msg, this);
  }
  terminate() { this.terminated = true; }
  emit(data) { this.onmessage?.({ data }); }
}

function fakeResources({ prefetchProgress = [] } = {}) {
  return {
    async fetchVerified(release, url, { sha256, size } = {}) {
      return { json: async () => ({
        memory_mb: 256,
        biber: "2.21",
        // Bare names, beside the descriptor -- the VM is hosted separately
        // from the LaTeX mirror now, so every path `vm.json` names is
        // relative to its own location rather than to a mirror root.
        objects: "objects/",
        boot: { ready: "LIBREPAPER_VM_READY" },
        files: {
          "libv86.js": { url: "libv86.js", sha256: "a", size: 1 },
          "v86.wasm": { url: "v86.wasm", sha256: "b", size: 1 },
          "seabios.bin": { url: "seabios.bin", sha256: "c", size: 1 },
          "vgabios.bin": { url: "vgabios.bin", sha256: "d", size: 1 },
          "bzimage": { url: "bzimage", sha256: "e", size: 1 },
          "fs.json": { url: "fs.json", sha256: "f", size: 1 },
        },
      }) };
    },
    async prefetch(release, entries, onProgress) {
      let done = 0;
      for (const entry of entries) { done += 1; onProgress?.({ done, total: entries.length, scope: "" }); prefetchProgress.push(entry.name); }
    },
  };
}

// `/api/config`'s `biberVm` field: the descriptor's own location and the
// digest `vm.js` verifies it against, hosted separately from any LaTeX
// mirror -- see `latex.js`'s `loadBiberVm`.
const VM_CONFIG = { url: "https://vm.example/biber-vm/rel1/vm.json", sha256: "z", size: 1 };

function setupVm({ resources } = {}) {
  vm._testing.reset();
  FakeWorker.handler = null;
  vm._testing.inject({
    hasWebAssembly: () => true,
    Worker: FakeWorker,
    navigator: () => ({ userAgent: "Mozilla/5.0" }),
    resources: resources || fakeResources(),
    // No scenario here exercises the real boot/job/idle timeouts (600s,
    // 120s, 5min); a real timer would just hold the Node process open
    // until the check exits. `testIdleTeardown` injects its own tracking
    // version to drive that timer by hand instead.
    setTimeout: () => 0,
    clearTimeout: () => {},
  });
}

async function prepareReady(opts) {
  setupVm(opts);
  FakeWorker.handler = (msg, worker) => {
    if (msg.type === "boot") worker.emit({ type: "status", status: "ready" });
  };
  const progress = [];
  await vm.prepare(VM_CONFIG, (p) => progress.push(p));
  return progress;
}

/* --------------------------------------------------------- prepare() */

async function testPrepareProgress() {
  const progress = await prepareReady();
  check("prepare() resolves to a ready VM", vm.state() === "ready", vm.state());
  check("progress reports the bibliography-support scope", progress.length > 0 && progress.every((p) => p.scope === "bibliography support"));
  check("prepare() boots exactly one worker", FakeWorker.current.posted.some((m) => m.type === "boot"));
  const config = FakeWorker.current.posted.find((m) => m.type === "boot").config;
  check(
    "boot config carries the runtime file URLs, resolved beside the descriptor",
    config.libv86Url === "https://vm.example/biber-vm/rel1/libv86.js" && config.wasmPath === "https://vm.example/biber-vm/rel1/v86.wasm",
    JSON.stringify(config),
  );
  check("boot config carries the guest's memory size", config.memoryBytes === 256 * 1024 * 1024);
}

async function testPrepareIdempotent() {
  await prepareReady();
  const postedBefore = FakeWorker.current.posted.length;
  await vm.prepare(VM_CONFIG, () => {});
  check("a second prepare() for the same VM does not reboot", FakeWorker.current.posted.length === postedBefore);
}

async function testPrepareUnsupported() {
  vm._testing.reset();
  vm._testing.inject({ hasWebAssembly: () => false });
  await rejects(vm.prepare(VM_CONFIG, () => {}), "VmUnsupported", "prepare() rejects on an unsupported browser");
}

async function testPrepareNoVmImage() {
  setupVm();
  await rejects(vm.prepare(null, () => {}), "VmUnavailable", "prepare() rejects with no bibliography VM configured");
  await rejects(vm.prepare({ url: "https://vm.example/vm.json" }, () => {}), "VmUnavailable", "prepare() rejects a config with no sha256");
}

/* --------------------------------------------------------- jobs */

function respondJob(worker, { id, exitCode = 0, bbl = "bbl-bytes", blg = "", incompatible = false }) {
  const enc = new TextEncoder();
  worker.emit({
    type: "job-result", id, exitCode,
    bbl: bbl == null ? null : enc.encode(bbl).buffer,
    blg: enc.encode(blg).buffer,
    incompatible, biberVersion: "2.21",
  });
}

async function testJobSuccessUnicode() {
  await prepareReady();
  const unicodeBbl = "\\entry{café}{article}{}\n  \\field{title}{Unicode \u00e9\u00e8\u4e2d\u6587}\n\\endentry\n";
  FakeWorker.handler = (msg, worker) => {
    if (msg.type === "job") respondJob(worker, { id: msg.id, bbl: unicodeBbl, blg: "INFO - done" });
  };
  const result = await vm.runBiber({ job: {}, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" });
  check("a clean job resolves ok", result.ok === true, JSON.stringify(result));
  check("Unicode BBL bytes survive exactly", new TextDecoder().decode(result.bbl) === unicodeBbl);
  check("the backend is vm", result.tool.backend === "vm");
}

async function testIncompatible() {
  await prepareReady();
  FakeWorker.handler = (msg, worker) => {
    if (msg.type === "job") respondJob(worker, { id: msg.id, exitCode: 2, bbl: null, blg: "control file version mismatch", incompatible: true });
  };
  const result = await vm.runBiber({ job: {}, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" });
  check("an incompatible control file is reported", result.incompatible === true);
  check("an incompatible job is not ok", result.ok === false);
}

async function testSuperseding() {
  await prepareReady();
  const held = [];
  FakeWorker.handler = (msg) => { if (msg.type === "job") held.push(msg.id); };
  const p1 = vm.runBiber({ job: {}, stem: "one", bcf: new Uint8Array([1]), files: {}, identity: "a" });
  await new Promise((r) => setTimeout(r, 0));
  const p2 = vm.runBiber({ job: {}, stem: "two", bcf: new Uint8Array([1]), files: {}, identity: "b" });
  const p3 = vm.runBiber({ job: {}, stem: "three", bcf: new Uint8Array([1]), files: {}, identity: "c" });
  await rejects(p2, "Superseded", "a queued job superseded by a newer one rejects as Superseded");
  respondJob(FakeWorker.current, { id: held[0] });
  const r1 = await p1;
  check("the running job is unaffected by superseding", r1.ok === true);
  await new Promise((r) => setTimeout(r, 0));
  respondJob(FakeWorker.current, { id: held[1] });
  const r3 = await p3;
  check("the surviving queued job runs after the first finishes", r3.ok === true);
}

async function testAbortCanceled() {
  await prepareReady();
  let canceledId = null;
  FakeWorker.handler = (msg, worker) => {
    if (msg.type === "job") return;
    if (msg.type === "cancel") { canceledId = msg.id; worker.emit({ type: "job-error", id: msg.id, error: "Canceled" }); }
  };
  const controller = new AbortController();
  const p = vm.runBiber({ job: {}, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" }, { signal: controller.signal });
  controller.abort();
  await rejects(p, "Canceled", "an aborted job that the worker confirms rejects as Canceled");
  check("the worker received the matching cancel id", canceledId != null);
}

async function testPoisoning() {
  await prepareReady();
  FakeWorker.handler = (msg, worker) => {
    if (msg.type === "job") return;
    if (msg.type === "cancel") worker.emit({ type: "poisoned" });
  };
  const controller = new AbortController();
  const p = vm.runBiber({ job: {}, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" }, { signal: controller.signal });
  controller.abort();
  await rejects(p, "VmUnavailable", "a poisoned guest rejects the in-flight job");
  check("a poisoned guest retires: state() returns cold", vm.state() === "cold", vm.state());
  check("a poisoned guest's worker is terminated", FakeWorker.current.terminated === true);
}

/* --------------------------------------------------------- idle teardown */

async function testIdleTeardown() {
  const timers = [];
  vm._testing.reset();
  FakeWorker.handler = null;
  vm._testing.inject({
    hasWebAssembly: () => true,
    Worker: FakeWorker,
    navigator: () => ({ userAgent: "Mozilla/5.0" }),
    resources: fakeResources(),
    setTimeout: (fn, ms) => { const id = timers.length; timers.push({ fn, ms, canceled: false }); return id; },
    clearTimeout: (id) => { if (timers[id]) timers[id].canceled = true; },
  });
  FakeWorker.handler = (msg, worker) => {
    if (msg.type === "boot") worker.emit({ type: "status", status: "ready" });
    if (msg.type === "job") respondJob(worker, { id: msg.id });
  };
  await vm.prepare(VM_CONFIG, () => {});
  await vm.runBiber({ job: {}, stem: "main", bcf: new Uint8Array([1]), files: {}, identity: "i" });
  const idle = timers.find((t) => t.ms === 5 * 60 * 1000 && !t.canceled);
  check("an idle-teardown timer is scheduled after a job finishes", idle != null);
  idle.fn();
  check("firing the idle timer retires the VM", vm.state() === "cold", vm.state());
}

/* --------------------------------------------------------- run */

const tests = [
  testValidateRelativePath,
  testParseDoneMarker,
  testDetectIncompatible,
  testWorkerCopiesAgree,
  testSupported,
  testPrepareProgress,
  testPrepareIdempotent,
  testPrepareUnsupported,
  testPrepareNoVmImage,
  testJobSuccessUnicode,
  testIncompatible,
  testSuperseding,
  testAbortCanceled,
  testPoisoning,
  testIdleTeardown,
];

for (const test of tests) {
  await test();
}
vm._testing.reset();

if (failures) {
  console.error(`latex-vm: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`latex-vm: ${tests.length} scenario(s) passed`);
