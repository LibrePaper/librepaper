// Behavioral race checks for the actual Reader functions. The small VM
// harness loads function bodies from Reader.svelte and supplies only their
// browser/server collaborators, so these checks exercise the shipped guards.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { diagnosticContext } from "../../src/lib/assistant-review.js";
import { createReaderBoot } from "../../src/lib/reader/boot.js";
import { createPendingChat } from "../../src/lib/reader/chat.js";
import { createReaderCollaboration } from "../../src/lib/reader/collaboration.js";
import { needsSourceRefresh } from "../../src/lib/reader/source-events.js";

const reader = readFileSync(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = reader.indexOf(start);
  assert.notEqual(from, -1, `Reader function not found: ${start}`);
  const to = reader.indexOf(end, from);
  assert.notEqual(to, -1, `Reader function boundary not found: ${end}`);
  return reader.slice(from, to);
};

const showCheckpoint = body("  async function showCheckpoint(sha)", "  async function nameCheckpoint");
// A ready frame must not replace an in-flight historical URL/navigation with
// an automatic captured-current comparison.
for (const [arrived, navigating, expected] of [["old", 0, 0], ["", 1, 0], ["", 0, 1]]) {
  let comparisons = 0;
  vm.runInNewContext(body('        if (panel === "history" && historyBaseline', '        if (first && !publishedMode)'), {
    panel: "history", historyBaseline: { sha: "old" }, historyComparePoint: null, viewing: null,
    ARRIVED_AT: arrived, checkpointNavigationPending: navigating,
    computeHistoryChanges: () => comparisons++,
  });
  assert.equal(comparisons, expected, "historical navigation owns frame readiness");
}
const backToNow = body("  function backToNow()", "  async function nameCheckpoint");
for (const stale of [false, true]) {
  let finish;
  const pending = new Promise((resolve) => { finish = resolve; });
  const delivered = [];
  const ctx = vm.createContext({
    historyController: { capturedCurrent: { sha: "captured", main: "main.md", texts: { "main.md": "frozen" } } },
    navigationGeneration: 0, issued: 0, readerDisposed: false, viewing: null,
    renderingStore: { invalidate: () => {} }, dropHeldRendering: () => {}, showMobileView: () => {},
    passages: { renderTree: () => pending }, SLUG: "doc", KEY: "", keyHeaders: () => ({}),
    framePreview: { publish: (value) => delivered.push(value) },
    paintPreview: () => { throw new Error("captured targets must not depend on the live preview queue"); },
  });
  vm.runInContext(body("  async function showCapturedCurrent()", "  let restoring"), ctx);
  const showing = ctx.showCapturedCurrent();
  if (stale) ctx.navigationGeneration++;
  finish({ html: "<p>frozen</p>" });
  await showing;
  assert.equal(delivered.length, stale ? 0 : 1);
  if (!stale) assert.equal(delivered[0].sha, "captured");
}
// `paintPreview` delegates its staleness guard and its figure fetch to two
// helpers defined next to it. They are sliced in with it so the five contexts
// below exercise the real guard rather than a stand-in -- which is the whole
// point of the coalescing and invalidation cases further down.
const previewHelpers = body("  function superseded(mine", "  // Outline reads the same live text");
// `updatePreviewTarget` points the local app at this document through a small
// helper now, so that helper is sliced in wherever the function is exercised.
const pairLocalQuarto = body("  function pairLocalQuarto()", "  // Which of Quarto's own live preview");
const paintPreview = `${previewHelpers}\n${body("  async function paintPreview()", "  // A source that is not actively")}`;

// Initial Quarto setup must configure the companion as active for an editor.
// If permission is assigned afterwards, editable onboarding examples remain
// on the Markdown fallback even when the local app is already paired.
{
  const prepare = body("  async function prepare(document_)", "  $effect(() => {");
  assert.ok(
    prepare.indexOf("mayEdit = Boolean(allowed)") < prepare.indexOf("pairLocalQuarto()"),
    "prepare resolves edit permission before configuring the Quarto companion",
  );
}

// Session lifecycle checks use the extracted resource owners directly. The
// fake room/session expose only the contracts Reader needs, which keeps these
// checks focused on stale metadata and teardown rather than WebSocket syntax.
function fakeSession(joinOptions) {
  const observers = new Set();
  return {
    joined: false,
    awareness: { on: (_name, fn) => observers.add(fn), off: (_name, fn) => observers.delete(fn) },
    watchSource: () => {}, onSwap: () => {}, onFiles: () => () => {},
    open: () => ({ type: "y-open" }), disconnected() { this.joined = false; },
    leave() { this.left = true; observers.clear(); },
    joinOptions,
  };
}

function fakeCollaboration() {
  let joined;
  return {
    module: { join(options) { joined = fakeSession(options); return joined; } },
    get session() { return joined; },
  };
}

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};

// A replaced boot cannot let old identity or document responses populate the
// new reader, and disposal suppresses both callbacks.
{
  const oldDocument = deferred();
  const newDocument = deferred();
  const oldIdentity = deferred();
  const newIdentity = deferred();
  const documents = [];
  const identities = [];
  let documentRequests = 0;
  let identityRequests = 0;
  const boot = createReaderBoot({
    slug: "example",
    fetcher: () => (++documentRequests === 1 ? oldDocument.promise : newDocument.promise),
    whoami: () => (++identityRequests === 1 ? oldIdentity.promise : newIdentity.promise),
    onDocument: (value) => documents.push(value),
    onIdentity: (value) => identities.push(value),
  });
  const first = boot.start();
  const second = boot.start();
  oldDocument.resolve({ ok: true, json: async () => ({ id: "old" }) });
  oldIdentity.resolve({ name: "old" });
  newDocument.resolve({ ok: true, json: async () => ({ id: "new" }) });
  newIdentity.resolve({ name: "new" });
  await Promise.all([first, second]);
  await new Promise(setImmediate);
  assert.deepEqual(documents, [{ id: "new" }]);
  assert.deepEqual(identities, [{ name: "new" }]);

  const late = deferred();
  let callbacks = 0;
  const disposed = createReaderBoot({ slug: "example", fetcher: () => late.promise, onDocument: () => callbacks++ });
  disposed.start();
  disposed.dispose();
  late.resolve({ ok: true, json: async () => ({ id: "late" }) });
  await new Promise(setImmediate);
  assert.equal(callbacks, 0);
}

const context = (values) => vm.createContext({
  clearTimeout,
  setTimeout,
  queueMicrotask,
  Promise,
  Uint8Array,
  ArrayBuffer,
  readerDisposed: false,
  mayEdit: true,
  publishedPublication: null,
  editing: true,
  latexOutput: "pdf",
  outlineRevision: 0,
  sourceFormat: "",
  typstOutput: "pdf",
  previewMain: "",
  previewFile: "",
  session: null,
  viewing: null,
  historyController: { noteLiveChange: () => {} },
  checkpointNavigationPending: 0,
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
    renderingStore: { invalidate: () => {}, cancelPoll: () => {}, reset: () => {}, schedulePoll: () => {} },
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
    renderingStore: { invalidate: () => {}, cancelPoll: () => {}, reset: () => {}, schedulePoll: () => {} },
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
    framePreview: { publish: () => {}, clear: () => {} },
    renderingStore: { cancelPoll: () => {} }, holdRendering: () => {},
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
  const mainText = {};
  const active = { files, text: mainText };
  const seenTransactions = new WeakSet();
  const ctx = context({ session: active, needsSourceRefresh, handledFileTransactions: seenTransactions, refreshFiles: () => refreshes++, sourceChanged: () => changes++ });
  vm.runInContext(body("  function filesChanged(events, active = session)", "  function refreshPeers()"), ctx);
  ctx.events = [{ target: {} }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(refreshes, 0);
  assert.equal(changes, 1);
  ctx.events = [{ target: mainText }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(changes, 1, "the source watcher owns main-text edits");
  ctx.events = [{ target: files }];
  vm.runInContext("filesChanged(events)", ctx);
  vm.runInContext("filesChanged({})", ctx);
  assert.equal(refreshes, 2);
  assert.equal(changes, 3);
  const transaction = {};
  ctx.events = [{ target: {}, transaction }];
  vm.runInContext("filesChanged(events)", ctx);
  ctx.events = [{ target: files, transaction }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(changes, 4, "one mixed Yjs transaction schedules one repaint");
  assert.equal(refreshes, 3);
  const other = { files: {}, text: {} };
  ctx.events = [{ target: {} }];
  ctx.other = other;
  vm.runInContext("filesChanged(events, other)", ctx);
  assert.equal(changes, 4, "stale session events are ignored");
}

// Renaming the main file into LaTeX configures the already joined project,
// even though the initial prepare ran while it was Markdown.
{
  let configured = 0;
  const ctx = context({
    session: {
      files: {}, list: () => [], folders: () => [], mainId: () => "main",
      mainPath: () => "paper.tex", latexSettings: () => ({ engine: "auto" }),
    },
    files: [], folders: [], openFile: "", previewMain: "", sourceFormat: "markdown", shownFigure: null,
    viewing: null, tick: async () => {}, paintPreview: () => {},
    mayEdit: true, previousFigure: null, ARRIVED_FILE: "", arrivedFileOpened: false,
    navigationGeneration: 0, renderingRequest: 0, issued: 0, rendering: null,
    renderingChecked: false, latestPreview: null, renderedSha: null,
    frameShowsCheckpoint: false, everPainted: false, everPaintedShown: false,
    docsOrigin: null, dropHeldRendering: () => {}, navigateFrame: () => {},
    framePreview: { clear: () => {} },
    renderingStore: { reset: () => {} },
    renderers: { formatOf: () => "latex", warm: () => {} },
    configureLatex: () => configured++, sourceChanged: () => {},
  });
  vm.runInContext(body("  function refreshFiles()", "  // A file added"), ctx);
  vm.runInContext("refreshFiles()", ctx);
  assert.equal(configured, 1);
}

// LaTeX configuration follows format/session lifetime. Shared settings reach
// the compiler once, and session changes do not trigger a release pin.
{
  const releases = [];
  const configurations = [];
  const updates = [];
  let paints = 0;
  const makeSession = () => {
    const observers = new Set();
    let settings = { engine: "auto" };
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
      // Compiler releases are mirror-global. Reader setup must not fetch a
      // release list or write an automatic project pin.
      releases: () => { throw new Error("reader must not load compiler releases"); },
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
  assert.equal(first.writes.length, 0);
  const second = makeSession();
  ctx.session = second;
  vm.runInContext('configureLatex("latex")', ctx);
  assert.equal(first.observers.size, 0);
  assert.equal(second.observers.size, 1);
  assert.equal(first.writes.length, 0);
  assert.equal(second.writes.length, 0);
  assert.equal(releases.length, 0, "readers must not load or pin a compiler release");
  vm.runInContext('stopLatex()', ctx);
  assert.equal(second.observers.size, 0);
  ctx.session = makeSession();
  ctx.mayEdit = false;
  vm.runInContext('configureLatex("latex")', ctx);
  assert.equal(configurations.at(-1).mayCompile, true, "cold readers compile from source without edit permission");
  assert.equal(releases.length, 0, "readers must not pin the default release");
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
    framePreview: { publish: (payload) => delivered.push(payload.html), clear: () => {} },
    renderingStore: { cancelPoll: () => {} },
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

// Peer updates notify history without repainting the frozen preview.
{
  let notified = 0;
  const ctx = context({
    sourceGeneration: 0, viewing: { sha: "old" }, mayEdit: true, publishedPublication: null,
    outlineRevision: 0,
    historyController: { noteLiveChange: () => notified++ },
    diagnosticPainter: { typed: () => assert.fail("historical preview must remain stable") },
  });
  vm.runInContext(body("  function sourceChanged()", "  /* ------------------------------------------------------- keeping in step */"), ctx);
  vm.runInContext("sourceChanged()", ctx);
  assert.equal(notified, 1);
  assert.equal(ctx.sourceGeneration, 1);
}

// Further keystrokes must not postpone an editor's already scheduled preview.
{
  let scheduled = 0;
  const ctx = context({
    sourceGeneration: 0, editing: true, sourceFormat: "markdown", pdfOutput: false, mayEdit: true, publishedPublication: null,
    previewTimer: null, PASSIVE_PREVIEW_DEBOUNCE: 1000,
    diagnosticPainter: { typed: () => {} },
    setTimeout: (fn, ms) => {
      assert.ok(ms <= 50, "Markdown starts rendering within 50 ms of an edit");
      return ++scheduled;
    }, clearTimeout: () => {}, paintPreview: () => {},
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
    setTimeout: (fn, ms) => {
      assert.ok(ms <= 50, "Typst starts rendering within 50 ms of an edit");
      return ++scheduled;
    }, clearTimeout: () => {}, paintPreview: () => {}, dropHeldRendering: () => {},
    renderingStore: { schedulePoll: () => {}, cancelPoll: () => {} },
  });
  vm.runInContext(body("  function sourceChanged()", "  /* ------------------------------------------------------- keeping in step */"), ctx);
  for (let i = 0; i < 10; i++) vm.runInContext("sourceChanged()", ctx);
  assert.equal(scheduled, 1, "Typst preview timer remains bounded during typing");
}

// Yjs may notify the main-text observer from inside CodeMirror's update
// listener. Diagnostic painting must wait until that update has returned;
// otherwise Editor.setDiagnostics dispatches a forbidden nested update.
{
  let inEditorUpdate = true;
  let paintedDiagnostics = 0;
  const ctx = context({
    sourceGeneration: 0, quartoFreshnessSerial: 0, sourceFormat: "quarto", quartoView: "draft",
    session: {
      mainPath: () => "main.qmd", mainId: () => "main", textOf: () => ({ toString: () => "# Draft\n" }),
      text: { toString: () => "# Draft\n" },
    },
    quarto: { parseQuarto: () => ({ diagnostics: [], source: "# Draft\n" }) },
    renderDiagnostics: [], paintCombinedDiagnostics: () => {
      if (inEditorUpdate) throw new Error("nested CodeMirror update");
      paintedDiagnostics++;
    },
    diagnosticPainter: { typed: () => {} }, diagnosticContext,
    editing: true, pdfOutput: false, previewTimer: null,
    setTimeout: () => {}, clearTimeout: () => {}, paintPreview: () => {},
  });
  vm.runInContext(body("  function sourceChanged()", "  /* ------------------------------------------------------- keeping in step */"), ctx);
  vm.runInContext("sourceChanged()", ctx);
  inEditorUpdate = false;
  await new Promise(queueMicrotask);
  assert.equal(paintedDiagnostics, 1, "diagnostics paint after the editor update returns");
}

// A Quarto document with no live preview running paints its Markdown draft
// like any other draft format -- nothing rendered is ever uploaded, so there
// is no saved bundle left to prefer over it.
{
  let compiled = 0;
  const published = [];
  const ctx = context({
    sourceFormat: "quarto", session: {}, paintsTheFrame: true, pdfOutput: false, compilesHere: false,
    issued: 0, painted: 0, viewing: null, navigationGeneration: 0, sourceGeneration: 0,
    previewPaintBusy: false, previewPaintQueued: false, previewTimer: null, everPainted: true,
    treeNow: () => ({ main: "main.qmd", texts: { "main.qmd": "source" }, digests: {} }),
    headingOf: async () => "Title",
    renderers: {
      formatOf: () => "quarto", producesPdf: () => false,
      render: async () => { compiled++; return { html: "<p>draft</p>", diagnostics: [] }; },
    },
    framePreview: { publish: (value) => published.push(value), clear: () => {} },
    renderingStore: { cancelPoll: () => {} },
    diagnosticPainter: { rendered: () => {} }, say: () => {},
  });
  vm.runInContext(paintPreview, ctx);
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(compiled, 1, "a Quarto document with no live preview compiles its own draft");
  assert.equal(published[0].kind, "html");
  assert.equal(published[0].html, "<p>draft</p>");
}

// A historical endpoint uses the contemporary HTML renderer with its captured
// tree, never the editor's ordinary renderer/PDF path. Settings and captured
// asset bytes must reach that endpoint unchanged.
{
  const sourceAsset = Uint8Array.of(4, 5, 6);
  const sourceTree = {
    main: "paper.typ",
    texts: { "paper.typ": "= Historical" },
    digests: { "figure.png": "digest-history" },
    settings: { release: "r1", engine: "typst" },
  };
  let renderedTree = null;
  let ordinaryRendererCalls = 0;
  const ctx = context({
    displayedFormat: "typst", sourceFormat: "typst", pdfOutput: false,
    compilesHere: false, paintsTheFrame: true, editing: false,
    issued: 0, painted: 0, viewing: { sha: "checkpoint" },
    navigationGeneration: 0, sourceGeneration: 0,
    previewPaintBusy: false, previewPaintQueued: false, previewTimer: null,
    everPainted: true, latestPreview: null,
    SLUG: "history-doc", KEY: "", authHeaders: () => ({}),
    keyHeaders: () => ({}),
    treeNow: () => sourceTree,
    headingOf: async () => "Historical",
    figures: {
      gather: async () => ({ assets: { "figure.png": sourceAsset.slice() }, urls: { "figure.png": "blob:history" } }),
    },
    passages: {
      renderTree: async (_slug, tree) => {
        renderedTree = tree;
        return { html: "<p>historical</p>", diagnostics: [] };
      },
    },
    renderers: {
      formatOf: () => "typst", producesPdf: () => true,
      render: () => { ordinaryRendererCalls += 1; return { pdf: Uint8Array.of(9), diagnostics: [] }; },
    },
    framePreview: { publish: (payload) => { ctx.published = payload; }, clear: () => {} },
    renderingStore: { cancelPoll: () => {} },
    diagnosticPainter: { rendered: () => {} }, say: () => {},
  });
  vm.runInContext(paintPreview, ctx);
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(ordinaryRendererCalls, 0, "historical preview bypasses the ordinary compiler");
  assert.equal(renderedTree.settings.release, "r1");
  assert.deepEqual([...renderedTree.assets["figure.png"]], [4, 5, 6]);
  assert.equal(ctx.published.html, "<p>historical</p>");
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
    framePreview: { publish: (payload) => delivered.push(payload.bytes[0]), clear: () => {} },
    renderingStore: { cancelPoll: () => {} },
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
  assert.equal(held.length, 0, "Typst output is transient and is never retained");
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
    framePreview: {
      publish: () => assert.fail("a failed first Typst compile must not deliver a PDF"),
      clear: () => {},
    },
    renderingStore: { cancelPoll: () => {} },
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
  ctx.renderers.render = async () => ({ pdf: null, log: "", diagnostics: [], failure: { message: "The mirror has no engine release" } });
  await vm.runInContext("paintPreview()", ctx);
  assert.equal(ctx.pdfFailureReason, "The mirror has no engine release");

  // HTML conversion errors belong to the source snapshot that caused them.
  const pendingHtml = deferred();
  ctx.latexOutput = "html";
  ctx.pdfOutput = false;
  ctx.pdfFailure = false;
  ctx.pdfFailureReason = "";
  ctx.renderers.render = (_tree, _title, options) => {
    assert.equal(options.format, "html");
    return pendingHtml.promise;
  };
  const htmlPaint = vm.runInContext("paintPreview()", ctx);
  await new Promise(setImmediate);
  ctx.sourceGeneration++;
  pendingHtml.reject(new Error("old conversion failed"));
  await htmlPaint;
  assert.equal(ctx.pdfFailure, false, "an old HTML error cannot mark a newer edit failed");
  assert.equal(ctx.pdfFailureReason, "");
}
console.log("reader-races: continuous preview, render coalescing and navigation guards passed");

// Ctrl-S says "saved on the server" only when that is true: never while
// updates are pending, never while the connection is down.
{
  const said = [];
  const ctx = context({
    connected: true, persistence: { pending: 1, local: true },
    say: (value) => { said.push(value); },
  });
  vm.runInContext(body("  function reportPersistence()", "  // There is no save, so a close"), ctx);
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, []);
  ctx.persistence.pending = 0;
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, ["saved on the server"]);
  ctx.connected = false;
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, ["saved on the server"]);
}

// Reconnect metadata is checked before the old session can send its CRDT.
// A late response from an older reconnect attempt cannot re-open the room.
{
  const first = deferred();
  const second = deferred();
  const sent = [];
  const states = [];
  const rooms = [];
  let fetches = 0;
  const fake = fakeCollaboration();
  const collaboration = createReaderCollaboration({
    slug: "example", fetcher: () => (++fetches === 1 ? first.promise : second.promise),
    openRoom: (_slug, options) => {
      rooms.push(options);
      return { send: (message) => sent.push(message), sendLive: () => ({ ok: true }), close: () => {} };
    },
    collab: fake.module,
    getCanEdit: () => true,
    onConnected: (up) => states.push(up),
  });
  collaboration.start({ created_at: "first", role: "editor" });
  const active = fake.session;
  active.joinOptions.send({ type: "y-update", update: "held" });
  assert.equal(sent.length, 1, "the initial y-open is sent");
  rooms[0].onConnected(false);
  const stale = rooms[0].onConnected(true);
  const current = rooms[0].onConnected(true);
  second.resolve({ ok: true, json: async () => ({ created_at: "first", role: "editor" }) });
  await current;
  first.resolve({ ok: true, json: async () => ({ created_at: "first", role: "editor" }) });
  await stale;
  assert.equal(sent.filter((message) => message.type === "y-open").length, 2, "only the current reconnect re-opens");
  collaboration.close();
  assert.equal(active.left, true);
}

// A recreated document or changed capability invalidates the old session and
// never sends its state vector back to the new server.
for (const latest of [
  { created_at: "second", role: "editor" },
  { created_at: "first", role: "reader" },
]) {
  const sent = [];
  let changed = 0;
  const fake = fakeCollaboration();
  let roomOptions;
  const collaboration = createReaderCollaboration({
    slug: "example", fetcher: async () => ({ ok: true, json: async () => latest }),
    openRoom: (_slug, options) => {
      roomOptions = options;
      return { send: (message) => sent.push(message), sendLive: () => ({ ok: true }), close: () => {} };
    },
    collab: fake.module,
    getCanEdit: () => true,
    onDocumentChanged: () => changed++,
  });
  collaboration.start({ created_at: "first", role: "editor" });
  roomOptions.onConnected(false);
  await roomOptions.onConnected(true);
  collaboration.close();
  assert.equal(changed, 1, "changed metadata invalidates the old session");
  assert.equal(sent.filter((message) => message.type === "y-open").length, 1);
}

// A metadata retry and a pending chat timeout are both cancelled by teardown.
{
  const timers = [];
  const fake = fakeCollaboration();
  let roomOptions;
  const collaboration = createReaderCollaboration({
    slug: "example", fetcher: async () => { throw new Error("offline"); },
    openRoom: (_slug, options) => {
      roomOptions = options;
      return { send: () => {}, sendLive: () => ({ ok: true }), close: () => {} };
    },
    collab: fake.module,
    retryMs: 1,
    setTimer: (fn) => { const id = timers.length + 1; timers.push({ id, fn, cleared: false }); return id; },
    clearTimer: (id) => { const timer = timers.find((item) => item.id === id); if (timer) timer.cleared = true; },
  });
  collaboration.start({ created_at: "first", role: "editor" });
  roomOptions.onConnected(false);
  await roomOptions.onConnected(true);
  collaboration.close();
  assert.equal(timers.length, 1, "metadata failure schedules one retry");
  assert.equal(timers[0].cleared, true, "teardown clears metadata retry");

  let now = 0;
  const chat = createPendingChat({
    createId: () => "chat-1", setTimer: (fn, ms) => { const id = ++now; timers.push({ id, fn, ms }); return id; },
    clearTimer: (id) => { const timer = timers.find((item) => item.id === id); if (timer) timer.cleared = true; },
    send: () => ({ ok: true }),
  });
  const timed = chat.send("hello");
  timers.at(-1).fn();
  assert.equal(await timed, false, "chat acknowledgement timeout settles the send");
  const result = chat.send("teardown");
  chat.dispose();
  assert.equal(await result, false, "teardown settles pending chat");
  assert.equal(timers[0].cleared, true, "teardown clears the chat timer");
}

// Opening an auxiliary file leaves the preview alone. Explicit preview
// selection changes its format without mutating the shared main.
{
  const texts = { 'paper.qmd': '# Quarto', 'notes.typ': '= Typst', 'other.typ': '= Other' };
  const files = Object.keys(texts).map(path => ({ id: path, path, kind: 'text' }));
  const configured = [];
  let cleared = 0, painted = 0;
  const ctx = context({
    session: { mainPath: () => 'paper.qmd', paths: new Map(files.map(f => [f.id, f.path])) },
    files, openFile: 'paper.qmd', previewMain: 'paper.qmd', sourceFormat: 'quarto',
    previewFile: '', canPreviewFile: true, compact: false, layout: 'split',
    viewing: null, editing: true, editor: { text: () => null },
    liveTreeNow: () => ({main:'paper.qmd',texts,digests:{}}),
    renderers: { formatOf: p => p.endsWith('.typ') ? 'typst' : 'quarto', warm: () => {} },
    configureLatex: f => configured.push(f), mayEdit: true,
    localQuarto: { configure: () => {}, bindingId: () => 'hosted', probe: async () => {} },
    SLUG: 'mixed', location: {origin:'http://localhost'},
    navigationGeneration: 0, issued: 0, docsOrigin: null,
    renderingStore: {reset: () => {}}, framePreview: {clear: () => cleared++},
    dropHeldRendering: () => {}, tick: async () => {}, paintPreview: () => painted++,
  });
  vm.runInContext(`${pairLocalQuarto}\n${body('  function updatePreviewTarget()', '  // A file added')}`, ctx);
  vm.runInContext(body('  function treeNow()', '  // Painting the preview'), ctx);
  for (const path of ['notes.typ', 'other.typ', 'paper.qmd']) {
    const previous = ctx.previewMain;
    ctx.openFile = path;
    vm.runInContext('updatePreviewTarget()', ctx);
    assert.equal(ctx.previewMain, previous, 'opening a file does not change the preview');
    vm.runInContext('previewThisFile()', ctx);
    assert.equal(ctx.previewMain, path);
    assert.equal(ctx.sourceFormat, path.endsWith('.typ') ? 'typst' : 'quarto');
    assert.equal(vm.runInContext('treeNow().main', ctx), path);
    assert.equal(ctx.session.mainPath(), 'paper.qmd');
    await new Promise(setImmediate);
  }
  assert.deepEqual(configured, ['typst','typst','quarto']);
  assert.equal(cleared, 3);
  assert.equal(painted, 3);
  assert.equal(texts['paper.qmd'], '# Quarto');
  ctx.canPreviewFile = false;
  ctx.openFile = 'data.csv';
  vm.runInContext('previewThisFile()', ctx);
  assert.equal(ctx.previewMain, 'paper.qmd', 'unsupported files cannot become preview targets');

  // A selected file keeps its identity through a rename and falls back to
  // the shared main when deleted.
  ctx.previewFile = 'notes.typ';
  files[1].path = 'renamed.typ';
  vm.runInContext('updatePreviewTarget()', ctx);
  assert.equal(ctx.previewMain, 'renamed.typ');
  ctx.files = files.filter(f => f.id !== 'notes.typ');
  vm.runInContext('updatePreviewTarget()', ctx);
  assert.equal(ctx.previewMain, 'paper.qmd');
}
console.log('reader-races: explicit preview selection, auxiliary files, rename and deletion passed');

// Initial live-file discovery must not cancel a historical link being fetched.
{
  const ctx = context({
    session: { mainPath: () => 'main.md' }, files: [],
    previewMain: '', sourceFormat: 'markdown', navigationGeneration: 7,
    checkpointNavigationPending: 7, mayEdit: false,
    renderers: { formatOf: () => 'markdown', warm: () => {} }, configureLatex: () => {},
  });
  vm.runInContext(`${pairLocalQuarto}\n${body('  function updatePreviewTarget()', '  function previewThisFile()')}`, ctx);
  vm.runInContext('updatePreviewTarget()', ctx);
  assert.equal(ctx.previewMain, 'main.md');
  assert.equal(ctx.navigationGeneration, 7, 'pending checkpoint navigation retains ownership');
}

// Main and secondary previews never expose the retired rendering store.
assert.doesNotMatch(reader, /createRenderingStore|holdRendering|\/renderings\//);

// Desktop View and the compact menu dispatch the same preview commands.
{
  const commands = [];
  const ctx = context({
    previewThisFile: () => commands.push('file'),
    setQuartoPreviewMode: mode => commands.push(mode),
    setTypstPreviewMode: mode => commands.push(mode),
    setLatexOutput: mode => commands.push(`latex-${mode}`),
    chose: value => commands.push(value),
    FILE_COMMANDS: [], chooseToolCommand: () => { throw Error('preview dispatched to Tools'); },
  });
  vm.runInContext(body('  function chooseViewCommand(value)', '  // The File menu.'), ctx);
  vm.runInContext(body('  function chooseCompactCommand(value)', '  /* ------------------------------------------------------------------- boot */'), ctx);
  vm.runInContext('chooseViewCommand("preview-file"); chooseViewCommand("preview-quarto"); chooseCompactCommand("preview-markdown"); chooseCompactCommand("preview-typst"); chooseCompactCommand("preview-calepin"); chooseViewCommand("layout-split")', ctx);
  assert.deepEqual(commands, ['file','quarto','markdown','typst','calepin','layout-split']);
  vm.runInContext('chooseViewCommand("preview-latex-html"); chooseCompactCommand("preview-latex-pdf")', ctx);
  assert.deepEqual(commands.slice(-2), ['latex-html', 'latex-pdf']);
  const view = body('{#snippet viewItems()}', '{#snippet toolItems()}');
  const tools = body('{#snippet toolItems()}', '<Nav {me} documentation={false}>');
  assert.match(view, /Preview this file/);
  assert.match(view, /@render previewItems\(\)/);
  assert.doesNotMatch(tools, /preview-/);
  assert.match(reader, /icon="eye" label="Preview this file"/);
}
console.log('reader-races: View and compact menus expose the same explicit preview controls');
