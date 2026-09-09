// Behavioral race checks for the actual Reader functions. The small VM
// harness loads function bodies from Reader.svelte and supplies only their
// browser/server collaborators, so these checks exercise the shipped guards.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { diagnosticContext } from "../src/lib/assistant-review.js";

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
const paintPreview = body("  async function paintPreview()", "  // Editors refresh at a bounded cadence");

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
  diagnosticContext,
  snapshotDigest: async () => "test-render-digest",
  historyDiffGeneration: 0,
  historyComparePoint: null,
  historyChanges: null,
  write: () => {},
  ...values,
});

// Selecting A and then B must leave B selected if A's history response is late.
{
  const a = deferred();
  const b = deferred();
  const ctx = context({
    navigationGeneration: 0, renderingRequest: 0, issued: 0, viewing: null,
    historyController: { invalidateChanges: () => {} },
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
    historyController: { invalidateChanges: () => {} },
    historyProblem: "", frameShowsCheckpoint: false, editing: false,
    sourceFormat: "html", framedSource: "live",
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
    displayedFormat: "latex", pdfOutput: true, compilesHere: true, paintsTheFrame: true,
    issued: 0, painted: 0, viewing: null, navigationGeneration: 0,
    sourceGeneration: 0, previewPaintBusy: false, previewPaintQueued: false,
    compiling: false, everPainted: true, latestPreview: old,
    sourceFormat: "latex", previewTimer: null, rendering: null,
    treeNow: () => ({ main: "paper.tex", texts: { "paper.tex": "x" }, digests: {} }),
    snapshotDigest: async () => "digest-A", headingOf: async () => "",
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
    renderers: {
      formatOf: () => "latex", render: () => rendered.promise,
      producesPdf: () => true,
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
  assert.equal(ctx.previewPaintBusy, false);
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

// Renaming the main file into LaTeX configures the already joined project,
// even though the initial prepare ran while it was Markdown.
{
  let configured = 0;
  const ctx = context({
    session: {
      files: {}, list: () => [], folders: () => [], mainId: () => "main",
      mainPath: () => "paper.tex", latexSettings: () => ({ engine: "auto", release: null }),
    },
    files: [], folders: [], openFile: "", sourceFormat: "markdown", shownFigure: null,
    mayEdit: true, previousFigure: null, ARRIVED_FILE: "", arrivedFileOpened: false,
    navigationGeneration: 0, renderingRequest: 0, issued: 0, rendering: null,
    renderingChecked: false, latestPreview: null, renderedSha: null,
    frameShowsCheckpoint: false, everPainted: false, everPaintedShown: false,
    docsOrigin: null, dropHeldRendering: () => {}, navigateFrame: () => {},
    renderers: { formatOf: () => "latex", warm: () => {} },
    configureLatex: () => configured++, sourceChanged: () => {},
  });
  vm.runInContext(body("  function refreshFiles()", "  // A file added"), ctx);
  vm.runInContext("refreshFiles()", ctx);
  assert.equal(configured, 1);
}

// LaTeX configuration follows format/session lifetime. Shared settings reach
// the compiler once, and a late mirror response cannot write into a later session.
{
  const releases = [];
  const configurations = [];
  const updates = [];
  let paints = 0;
  const makeSession = () => {
    const observers = new Set();
    let settings = { engine: "auto", release: null };
    return {
      observers, writes: [],
      meta: { observe: (fn) => observers.add(fn), unobserve: (fn) => observers.delete(fn) },
      latexSettings: () => settings,
      setLatexSettings(value) { this.writes.push(value); settings = { ...settings, ...value }; },
      change(value) {
        settings = { ...settings, ...value };
        for (const fn of observers) fn({ changes: { keys: new Map([["latex.engine", {}]]) } });
      },
    };
  };
  const first = makeSession();
  const ctx = context({
    session: first, mayEdit: true, sourceFormat: "latex", SLUG: "project",
    renderers: { available: () => true }, paintPreview: () => paints++,
    latex: {
      configure: (value) => configurations.push(value), cancel: () => {},
      setSettings: (value) => updates.push(value),
      releases: () => { const pending = deferred(); releases.push(pending); return pending.promise; },
    },
  });
  vm.runInContext(body("  let latexObservedSession = null;", "  // The most recent LaTeX compile result"), ctx);
  vm.runInContext('configureLatex("latex"); configureLatex("latex")', ctx);
  assert.equal(configurations.length, 1);
  assert.equal(configurations[0].project, "project");
  assert.equal(configurations[0].mayCompile, true);
  assert.equal(first.observers.size, 1);
  first.change({ engine: "xelatex" });
  assert.equal(updates[0].engine, "xelatex");
  assert.equal(paints, 1);
  vm.runInContext('configureLatex("markdown")', ctx);
  assert.equal(first.observers.size, 0);
  vm.runInContext('configureLatex("latex")', ctx);
  assert.equal(first.observers.size, 1);
  releases[0].resolve({ default: "stale" });
  await Promise.resolve();
  assert.equal(first.writes.length, 0);
  const second = makeSession();
  ctx.session = second;
  vm.runInContext('configureLatex("latex")', ctx);
  assert.equal(first.observers.size, 0);
  assert.equal(second.observers.size, 1);
  releases[1].resolve({ default: "old-session" });
  await Promise.resolve();
  assert.equal(first.writes.length, 0);
  assert.equal(second.writes.length, 0);
  releases[2].resolve({ default: "current" });
  await Promise.resolve();
  assert.equal(second.writes[0].release, "current");
  vm.runInContext('stopLatex()', ctx);
  assert.equal(second.observers.size, 0);
  ctx.session = makeSession();
  ctx.mayEdit = false;
  vm.runInContext('configureLatex("latex")', ctx);
  assert.equal(configurations.at(-1).mayCompile, false);
  assert.equal(releases.length, 3, "readers must not pin the default release");
  vm.runInContext('stopLatex()', ctx);
}

// A worker result still advances the preview after a keystroke. Requests made
// during compilation coalesce into one render of the latest source.
for (const invalidate of [null, "navigation", "main"]) {
  const first = deferred();
  const second = deferred();
  const calls = [];
  const delivered = [];
  const diagnostics = [];
  let text = "first";
  let main = "main.md";
  const ctx = context({
    displayedFormat: "markdown", pdfOutput: false, compilesHere: false, paintsTheFrame: true,
    issued: 0, painted: 0, viewing: null, navigationGeneration: 0,
    sourceGeneration: 0, previewPaintBusy: false, previewPaintQueued: false,
    previewTimer: null, everPainted: true, latestPreview: null,
    treeNow: () => ({ main, texts: { [main]: text }, digests: {} }),
    headingOf: async () => "Title",
    renderers: {
      formatOf: () => "markdown",
      producesPdf: () => false,
      render: (tree) => {
        calls.push(tree);
        return calls.length === 1 ? first.promise : second.promise;
      },
    },
    deliverPreview: (payload) => delivered.push(payload.html),
    diagnosticPainter: { rendered: (value) => diagnostics.push(value) },
    say: (message) => assert.fail(message),
  });
  vm.runInContext(paintPreview, ctx);
  const pending = vm.runInContext("paintPreview()", ctx);
  await Promise.resolve();
  text = "intermediate";
  ctx.sourceGeneration++;
  await vm.runInContext("paintPreview()", ctx);
  text = "latest";
  ctx.sourceGeneration++;
  if (invalidate === "navigation") ctx.navigationGeneration++;
  if (invalidate === "main") main = "other.md";
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(calls.length, 1, "only one compile can be in flight");
  first.resolve({ html: "first", diagnostics: [] });
  await pending;
  await new Promise(setImmediate);
  assert.deepEqual(delivered, invalidate ? [] : ["first"]);
  assert.equal(diagnostics.length, 0, "outdated diagnostics stay hidden");
  assert.equal(calls.length, 2);
  assert.equal(calls[1].texts[main], "latest");
  second.resolve({ html: "latest", diagnostics: [] });
  await new Promise(setImmediate);
  assert.equal(delivered.at(-1), "latest");
  assert.equal(ctx.previewPaintBusy, false);
}

// Further keystrokes must not postpone an editor's already scheduled preview.
{
  let scheduled = 0;
  const ctx = context({
    sourceGeneration: 0, editing: true, sourceFormat: "markdown", pdfOutput: false,
    previewTimer: null, READER_DEBOUNCE: 1000,
    diagnosticPainter: { typed: () => {} },
    setTimeout: () => ++scheduled, clearTimeout: () => {}, paintPreview: () => {},
  });
  vm.runInContext(body("  function sourceChanged()", "  /* ------------------------------------------------------- keeping in step */"), ctx);
  for (let i = 0; i < 10; i++) vm.runInContext("sourceChanged()", ctx);
  assert.equal(scheduled, 1);
  assert.equal(ctx.sourceGeneration, 10);
}

// Typst's PDF compiler is still scheduled at a bounded cadence: continuous
// typing must not keep moving the timer's deadline forever.
{
  let scheduled = 0;
  const ctx = context({
    sourceGeneration: 0, editing: true, sourceFormat: "typst", pdfOutput: true, compilesHere: true,
    previewTimer: null, diagnosticPainter: { typed: () => {} },
    setTimeout: () => ++scheduled, clearTimeout: () => {}, paintPreview: () => {}, dropHeldRendering: () => {},
  });
  vm.runInContext(body("  function sourceChanged()", "  /* ------------------------------------------------------- keeping in step */"), ctx);
  for (let i = 0; i < 10; i++) vm.runInContext("sourceChanged()", ctx);
  assert.equal(scheduled, 1, "Typst preview timer remains bounded during typing");
}

// Typst follows the PDF lifecycle too: one compile in flight, latest request
// queued, and an older PDF cannot replace the newer source snapshot.
{
  const first = deferred();
  const second = deferred();
  const calls = [];
  const delivered = [];
  const held = [];
  const diagnostics = [];
  let text = "first";
  const ctx = context({
    displayedFormat: "typst", pdfOutput: true, compilesHere: true, paintsTheFrame: true,
    issued: 0, painted: 0, viewing: null, navigationGeneration: 0,
    sourceGeneration: 0, previewPaintBusy: false, previewPaintQueued: false,
    previewTimer: null, everPainted: false, latestPreview: null,
    paintRendering: async () => {},
    treeNow: () => ({ main: "main.typ", texts: { "main.typ": text }, digests: {} }),
    headingOf: async () => "Title", snapshotDigest: async () => "digest-typst",
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
    renderers: {
      formatOf: () => "typst",
      producesPdf: () => true,
      render: (tree) => {
        calls.push(tree);
        return calls.length === 1 ? first.promise : second.promise;
      },
    },
    deliverPreview: (payload) => delivered.push(payload.bytes[0]),
    holdRendering: (...args) => held.push(args),
    diagnosticPainter: { rendered: (value) => diagnostics.push(value) },
    tell: () => {}, say: (message) => assert.fail(message),
  });
  vm.runInContext(paintPreview, ctx);
  const pending = vm.runInContext("paintPreview()", ctx);
  await new Promise(setImmediate);
  text = "latest";
  ctx.sourceGeneration++;
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(calls.length, 1, "Typst keeps one PDF compile active");
  first.resolve({ pdf: Uint8Array.of(1), diagnostics: [] });
  await pending;
  await new Promise(setImmediate);
  assert.equal(calls.length, 2, "Typst queues the latest source snapshot");
  second.resolve({ pdf: Uint8Array.of(2), diagnostics: [] });
  await new Promise(setImmediate);
  assert.deepEqual(delivered, [1, 2], "Typst may show an intermediate PDF, then the latest one");
  assert.equal(held.length, 1, "only the current Typst snapshot is eligible for storage");
  assert.equal(diagnostics.length, 1, "an intermediate Typst PDF cannot clear newer diagnostics");
}

// A first Typst compile that returns structured diagnostics must leave the
// reader in its explicit PDF failure state instead of a blank frame.
{
  const ctx = context({
    displayedFormat: "typst", pdfOutput: true, compilesHere: true, paintsTheFrame: true,
    issued: 0, painted: 0, viewing: null, navigationGeneration: 0, sourceGeneration: 0,
    previewPaintBusy: false, previewPaintQueued: false, previewTimer: null,
    everPainted: false, everPaintedShown: false, latestPreview: null, pdfFailure: false,
    sourceFormat: "typst", treeNow: () => ({ main: "main.typ", texts: { "main.typ": "bad" }, digests: {} }),
    headingOf: async () => "Title", snapshotDigest: async () => "digest-typst",
    paintRendering: async () => {},
    figures: { gather: async () => ({ assets: {}, urls: {} }) },
    renderers: {
      formatOf: () => "typst", producesPdf: () => true,
      render: async () => ({ pdf: null, diagnostics: [{ severity: "error", message: "broken" }] }),
      failurePage: async () => null,
    },
    deliverPreview: () => assert.fail("a failed first Typst compile must not deliver a PDF"),
    holdRendering: () => assert.fail("a failed first Typst compile must not store a PDF"),
    diagnosticPainter: { rendered: () => {} }, tell: () => {}, say: (message) => assert.fail(message),
  });
  vm.runInContext(paintPreview, ctx);
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(ctx.pdfFailure, true, "structured Typst diagnostics set the PDF failure state");
  assert.equal(ctx.latestPreview, null, "a failed first Typst compile leaves no blank success preview");

  // A compiler setup failure has neither a TeX log nor source diagnostics.
  // Its structured reason must reach the failure pane.
  ctx.sourceFormat = "latex";
  ctx.displayedFormat = "latex";
  ctx.treeNow = () => ({ main: "main.tex", texts: { "main.tex": "source" }, digests: {} });
  ctx.renderers.formatOf = () => "latex";
  ctx.renderers.render = async () => ({ pdf: null, log: "", diagnostics: [], failure: { message: "The mirror has no WasmTex release" } });
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(ctx.pdfFailureReason, "The mirror has no WasmTex release");
}
console.log("reader-races: continuous preview, render coalescing and navigation guards passed");

// Ctrl-S uses the existing reactive saving/offline badge. A static copy would
// remain stuck after the server acknowledges the pending updates.
{
  const timers = [];
  const ctx = context({
    connected: true, persistence: { pending: 1, local: true }, state: "",
    say: (value) => { ctx.state = value; }, setTimeout: (fn) => timers.push(fn),
  });
  vm.runInContext(body("  function reportPersistence()", "  // There is no save, so a close"), ctx);
  vm.runInContext("reportPersistence()", ctx);
  assert.equal(ctx.state, "");
  ctx.persistence.pending = 0;
  vm.runInContext("reportPersistence()", ctx);
  assert.equal(ctx.state, "saved on the server");
  timers.pop()();
  assert.equal(ctx.state, "");
  ctx.connected = false;
  vm.runInContext("reportPersistence()", ctx);
  assert.equal(ctx.state, "");
}

// A reconnect must check the document's creation before sending the old CRDT.
for (const outcome of ["same", "recreated", "disconnected", "promoted", "downgraded"]) {
  const metadata = deferred();
  const sent = [];
  let reloads = 0;
  const active = { joined: true, disconnected() { this.joined = false; }, open: () => ({ type: "y-open" }) };
  const ctx = context({
    rejoinRequest: 0, connected: true, session: active,
    pendingChat: new Map(), settleChat: () => {},
    passages: { clearPassageCache: () => {} },
    doc: { created_at: "first", ...((outcome === "promoted" || outcome === "downgraded") ? { role: outcome === "promoted" ? "reader" : "editor" } : {}) }, SLUG: "example", KEY: "", keyHeaders: () => ({}),
    fetch: () => metadata.promise, location: { reload: () => reloads++ },
    room: { send: (message) => sent.push(message) }, outbox: { disconnected: () => {} },
  });
  vm.runInContext(body("  async function reconnected(up)", "  $effect(() => {\n    markViewed"), ctx);
  const reconnecting = vm.runInContext("reconnected(true)", ctx);
  assert.equal(active.joined, false);
  assert.equal(sent.length, 0);
  if (outcome === "disconnected") await vm.runInContext("reconnected(false)", ctx);
  metadata.resolve({ ok: true, json: async () => ({
    created_at: outcome === "recreated" ? "second" : "first",
    ...((outcome === "promoted" || outcome === "downgraded") ? {
      role: outcome === "promoted" ? "editor" : "reader",
    } : {}),
  }) });
  await reconnecting;
  assert.equal(sent.length, outcome === "same" ? 1 : 0);
  assert.equal(reloads, ["recreated", "promoted", "downgraded"].includes(outcome) ? 1 : 0);
}

// A missing historical PDF clears the old pages and records an honest
// missing rendering; repeated polls do not reload the frame forever.
{
  let navigations = 0;
  const ctx = context({
    viewing: { sha: "checkpoint", at: "today" }, renderingRequest: 0,
    rendering: null, renderedSha: "live", latestPreview: { kind: "pdf", sha: "live" },
    frameReady: true, frameEpoch: 0, frameReadyEpoch: 0, frameKind: "pdf",
    everPainted: true, everPaintedShown: true, frameShowsCheckpoint: true,
    previewTimer: null, RENDERING_POLL: 30_000,
    SLUG: "doc", KEY: "", SHELL_HEADERS: {}, keyHeaders: () => ({}),
    fetch: async () => ({ ok: false, arrayBuffer: async () => null }),
    navigateFrame: () => navigations++, deliverPreview: () => {},
  });
  vm.runInContext(paintRendering, ctx);
  await vm.runInContext("paintRendering()", ctx);
  assert.equal(ctx.rendering.missing, true);
  assert.equal(ctx.latestPreview, null);
  assert.equal(navigations, 1);
  await vm.runInContext("paintRendering()", ctx);
  assert.equal(navigations, 1);
}

// A document's first rendering is stored the moment a compile succeeds; every
// later one waits for the quiet minute. The server is asked once whether a
// rendering exists, and an edit while it is being asked drops what was held.
{
  const holdRendering = body("  async function noRenderingYet()", "  function dropHeldRendering()");
  const run = async ({ rendering, latest, current = true, editDuringAsk = false }) => {
    const stored = [];
    const timers = [];
    let asked = 0;
    const ctx = context({
      rendering, renderingChecked: false, heldRendering: null, renderingTimer: null,
      RENDERING_QUIET: 60_000, SLUG: "doc", KEY: "", SHELL_HEADERS: {}, keyHeaders: () => ({}),
      storeHeldRendering: () => stored.push(ctx.heldRendering),
      fetch: async () => {
        asked++;
        if (editDuringAsk) ctx.heldRendering = null;
        return { ok: true, json: async () => latest };
      },
      setTimeout: (_fn, ms) => timers.push(ms),
    });
    vm.runInContext(holdRendering, ctx);
    await vm.runInContext(`holdRendering("sha1", new ArrayBuffer(4), null, ${current})`, ctx);
    return { stored, timers, asked };
  };
  const first = await run({ rendering: null, latest: { live: "sha1" } });
  assert.equal(first.stored.length, 1, "the first rendering is stored at once");
  assert.equal(first.stored[0]?.name, "sha1");
  assert.equal(first.timers.length, 0);
  const later = await run({ rendering: null, latest: { live: "sha1", sha: "sha0" } });
  assert.equal(later.stored.length, 0, "a document with a rendering waits for the quiet minute");
  assert.deepEqual(later.timers, [60_000]);
  const known = await run({ rendering: { sha: "sha0" }, latest: { live: "sha1" } });
  assert.equal(known.asked, 0, "a rendering this page already knows of is not asked about");
  assert.deepEqual(known.timers, [60_000]);
  const stale = await run({ rendering: null, latest: { live: "sha1" }, current: false });
  assert.equal(stale.stored.length, 0, "a compile of a text that has moved on is never the first rendering");
  const edited = await run({ rendering: null, latest: { live: "sha1" }, editDuringAsk: true });
  assert.equal(edited.stored[0], null, "an edit while asking drops what was held");
}
console.log("reader-races: the first rendering is stored at once, later ones after the quiet minute");
