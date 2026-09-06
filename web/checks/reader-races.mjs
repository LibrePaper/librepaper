// Behavioral race checks for the actual Reader functions. The small VM
// harness loads function bodies from Reader.svelte and supplies only their
// browser/server collaborators, so these checks exercise the shipped guards.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const reader = readFileSync(new URL("../src/components/Reader.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = reader.indexOf(start);
  assert.notEqual(from, -1, `Reader function not found: ${start}`);
  const to = reader.indexOf(end, from);
  assert.notEqual(to, -1, `Reader function boundary not found: ${end}`);
  return reader.slice(from, to);
};

const showCheckpoint = body("  async function showCheckpoint(sha)", "  async function nameCheckpoint");
const backToNow = body("  function backToNow()", "  async function nameCheckpoint");
const paintRendering = body("  async function paintRendering()", "  // The PDF this browser compiled");
const deliverAndReplay = body("  function deliverPreview(payload)", "  // The kind of frame follows");
const paintPreview = body("  async function paintPreview()", "  // Wait for a brief typing pause");

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => (resolve = done));
  return { promise, resolve };
};

const context = (values) => vm.createContext({
  clearTimeout,
  setTimeout,
  Promise,
  Uint8Array,
  ArrayBuffer,
  ...values,
});

// Selecting A and then B must leave B selected if A's history response is late.
{
  const a = deferred();
  const b = deferred();
  const ctx = context({
    navigationGeneration: 0, renderingRequest: 0, issued: 0, viewing: null,
    historyProblem: "", frameShowsCheckpoint: false,
    SLUG: "doc", KEY: "key", keyHeaders: () => ({}), dropHeldRendering: () => {},
    paintPreview: () => Promise.resolve(),
    history: { checkpoint: (_slug, sha) => (sha === "A" ? a.promise : b.promise) },
  });
  vm.runInContext(`${showCheckpoint}\n${backToNow}`, ctx);
  const pa = vm.runInContext("showCheckpoint('A')", ctx);
  const pb = vm.runInContext("showCheckpoint('B')", ctx);
  b.resolve({ sha: "B" });
  await pb;
  a.resolve({ sha: "A" });
  await pa;
  assert.equal(ctx.viewing.sha, "B");
}

// Back-to-now invalidates an in-flight checkpoint before its response arrives.
{
  const a = deferred();
  const ctx = context({
    navigationGeneration: 0, renderingRequest: 0, issued: 0, viewing: null,
    historyProblem: "", frameShowsCheckpoint: false, editing: false,
    sourceFormat: "html", visibility: "public", framedSource: "live",
    SLUG: "doc", KEY: "key", keyHeaders: () => ({}), dropHeldRendering: () => {},
    paintPreview: () => Promise.resolve(), navigateFrame: () => {},
    history: { checkpoint: () => a.promise },
  });
  vm.runInContext(`${showCheckpoint}\n${backToNow}`, ctx);
  const pending = vm.runInContext("showCheckpoint('A')", ctx);
  vm.runInContext("backToNow()", ctx);
  a.resolve({ sha: "A" });
  await pending;
  assert.equal(ctx.viewing, null);
}

// The PDF bytes for an older request cannot overwrite a newer request.
{
  const first = deferred();
  const second = deferred();
  const requests = [];
  const sent = [];
  const ctx = context({
    viewing: { sha: "A", at: "a" }, renderingRequest: 0, renderedSha: null,
    rendering: null, latestPreview: null, previewTimer: null,
    frameReady: true, frameReadyEpoch: 0, frameEpoch: 0, frameKind: "pdf",
    frameShowsCheckpoint: false, everPainted: false, everPaintedShown: false,
    paintsTheFrame: true, tell: (message) => sent.push(message),
    deliverPreview: () => {},
    SLUG: "doc", KEY: "key", SHELL_HEADERS: {}, keyHeaders: () => {},
    paintPreview: () => {}, RENDERING_POLL: 30_000,
    fetch: async () => {
      const response = requests.length ? second : first;
      requests.push(response);
      return { ok: true, arrayBuffer: () => response.promise };
    },
  });
  vm.runInContext(paintRendering, ctx);
  const pa = vm.runInContext("paintRendering()", ctx);
  await Promise.resolve();
  ctx.viewing = { sha: "B", at: "b" };
  const pb = vm.runInContext("paintRendering()", ctx);
  await Promise.resolve();
  second.resolve(Uint8Array.of(2).buffer);
  await pb;
  first.resolve(Uint8Array.of(1).buffer);
  await pa;
  assert.equal(ctx.latestPreview.sha, "B");
  assert.equal(ctx.latestPreview.bytes[0], 2);
}

// A stored PDF fetched before iframe readiness is replayed by that iframe.
{
  const sent = [];
  const ctx = context({
    viewing: { sha: "A", at: "a" }, renderingRequest: 0, renderedSha: null,
    rendering: null, latestPreview: null, previewTimer: null,
    frameReady: false, frameReadyEpoch: 0, frameEpoch: 0, frameKind: "pdf",
    frameShowsCheckpoint: false, everPainted: false, everPaintedShown: false,
    paintsTheFrame: true, tell: (message) => sent.push(message),
    SLUG: "doc", KEY: "key", SHELL_HEADERS: {}, keyHeaders: () => {},
    paintPreview: () => {}, RENDERING_POLL: 30_000,
    fetch: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(7).buffer }),
  });
  vm.runInContext(`${deliverAndReplay}\n${paintRendering}`, ctx);
  await vm.runInContext("paintRendering()", ctx);
  assert.equal(sent.length, 0);
  ctx.frameReady = true;
  assert.equal(vm.runInContext("replayPreview()", ctx), true);
  assert.equal(sent.length, 1);
  assert.equal(ctx.renderedSha, "A");
}

// A compiler result with neither html nor pdf leaves the existing preview up.
{
  const rendered = deferred();
  const sent = [];
  const old = { kind: "html", html: "<p>last good page</p>" };
  const ctx = context({
    displayedFormat: "latex", compilesHere: true, paintsTheFrame: true,
    issued: 0, painted: 0, viewing: null, navigationGeneration: 0,
    sourceGeneration: 0, latexPaintBusy: false, latexPaintQueued: false,
    compiling: false, everPainted: true, latestPreview: old,
    sourceFormat: "latex", previewTimer: null, rendering: null,
    treeNow: () => ({ main: "paper.tex", texts: { "paper.tex": "x" }, digests: {} }),
    snapshotDigest: async () => "digest-A", headingOf: async () => "",
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
    renderers: {
      formatOf: () => "latex", render: () => rendered.promise,
      failurePage: async () => "<p>failure</p>",
    },
    SHELL_HEADERS: {}, KEY: "key", SLUG: "doc", keyHeaders: () => {},
    deliverPreview: () => {}, holdRendering: () => {},
    diagnosticPainter: { rendered: () => {} }, tell: (message) => sent.push(message),
    say: () => {},
  });
  vm.runInContext(paintPreview, ctx);
  const pending = vm.runInContext("paintPreview()", ctx);
  // Disabling editing while the compiler is active must release its latch.
  ctx.compilesHere = false;
  rendered.resolve({ pdf: null, diagnostics: [{ severity: "error" }] });
  await pending;
  assert.equal(ctx.latestPreview, old);
  assert.equal(sent.length, 0);
  assert.equal(ctx.latexPaintBusy, false);
}

console.log("reader-races: all checks passed");

// A nested edit schedules the preview without rebuilding the directory.
{
  let refreshes = 0;
  let changes = 0;
  const files = {};
  const ctx = context({ session: { files }, refreshFiles: () => refreshes++, sourceChanged: () => changes++ });
  vm.runInContext(body("  function filesChanged(events)", "  function refreshPeers()"), ctx);
  ctx.events = [{ target: {} }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(refreshes, 0);
  assert.equal(changes, 1);
  ctx.events = [{ target: files }];
  vm.runInContext("filesChanged(events)", ctx);
  vm.runInContext("filesChanged({})", ctx);
  assert.equal(refreshes, 2);
  assert.equal(changes, 3);
}
