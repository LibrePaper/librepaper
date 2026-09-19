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
import { createPreviewRenderer } from "../../src/lib/reader/preview-render.js";
import { createRenderCoordinator } from "../../src/lib/reader/render-coordinator.js";
import { loadRunes } from "../helpers/runes.mjs";
import { needsSourceRefresh } from "../../src/lib/reader/source-events.js";

const { createBuildSettings } = await loadRunes(new URL("../../src/lib/reader/build-settings.svelte.js", import.meta.url));
const { createTimeline } = await loadRunes(new URL("../../src/lib/reader/timeline.svelte.js", import.meta.url));

const reader = readFileSync(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
const body = (start, end) => {
  const from = reader.indexOf(start);
  assert.notEqual(from, -1, `Reader function not found: ${start}`);
  const to = reader.indexOf(end, from);
  assert.notEqual(to, -1, `Reader function boundary not found: ${end}`);
  return reader.slice(from, to);
};

// `updatePreviewTarget` scopes the local app to this document through small
// helpers now, so they are sliced in wherever that function is exercised.
const localAppHelpers = body("  /// Which document this browser tab is about", "  // Which engine draws each format");

// Initial Quarto setup must configure the companion as active for an editor.
// If permission is assigned afterwards, editable onboarding examples remain
// on the Markdown fallback even when the local app is already paired.
{
  const prepare = body("  async function prepare(document_)", "  $effect(() => {");
  assert.ok(
    prepare.indexOf("mayEdit = Boolean(allowed)") < prepare.indexOf("scopeLocalApp()"),
    "prepare resolves edit permission before scoping the local app, whose pairing is active only for an editor",
  );
}

// Session lifecycle checks use the extracted resource owners directly. The
// fake room/session expose only the contracts Reader needs, which keeps these
// checks focused on stale metadata and teardown rather than WebSocket syntax.
function fakeSession(joinOptions) {
  const observers = new Set();
  return {
    joined: false,
    // EphemeralStore returns the unsubscribe function rather than taking the
    // handler back later, so the fake returns one too.
    ephemeral: { subscribe: (fn) => { observers.add(fn); return () => observers.delete(fn); } },
    watchSource: () => {}, onSwap: () => {}, onFiles: () => () => {},
    open: () => ({ type: "doc-open" }), disconnected() { this.joined = false; },
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

// A failed metadata request can open an explicitly prepared local project;
// an ordinary cache miss still follows the normal error path.
{
  const documents = [];
  const errors = [];
  const boot = createReaderBoot({
    slug: "offline",
    fetcher: async () => { throw new Error("network down"); },
    whoami: async () => ({}),
    findLocal: async () => ({ document: { title: "Local paper", role: "editor" } }),
    onDocument: (value) => documents.push(value),
    onError: (error) => errors.push(error.message),
  });
  await boot.start();
  assert.equal(documents[0].title, "Local paper");
  assert.equal(documents[0].offline_prepared, true);
  assert.deepEqual(errors, []);
}

const context = (values) => {
  const ctx = vm.createContext({
    clearTimeout,
    setTimeout,
    queueMicrotask,
    Promise,
    Uint8Array,
    ArrayBuffer,
    readerDisposed: false,
    // Recomputing the review is part of what an edit does now; the blocks
    // that care override this to count the calls.
    refreshReview: () => {},
    // An edit also puts the readers' copy of the document back on the clock.
    // The block that cares overrides this to count the calls.
    bundle: { keepCurrent: () => {} },
    mayEdit: true,
    publishedBundle: null,
    editing: true,
    latexOutput: "pdf",
    outlineRevision: 0,
    sourceFormat: "",
    typstOutput: "pdf",
    previewMain: "",
    previewFile: "",
    SLUG: "doc",
    buildPreferences: { selection: "automatic", backend: "auto", output: "html" },
    session: null,
    diagnosticContext,
    snapshotDigest: async () => "test-render-digest",
    write: () => {},
    ...values,
  });
  ctx.renderCoordinator = createRenderCoordinator({
    navigation: () => ctx.navigationGeneration,
    source: () => ctx.sourceGeneration,
    main: () => ctx.treeNow?.().main || "",
    render: (ticket) => ctx.renderPreview(ticket),
  });
  ctx.renderState = {
    compiling: false, lastCompile: 0, failure: false, failureReason: "",
    lastLatexResult: null, provenance: null, docxArtifact: null,
  };
  ctx.renderStatus = {
    begin: ({ clearFailure = false } = {}) => {
      ctx.renderState.compiling = true;
      if (clearFailure) ctx.renderState.failure = false;
    },
    finish: () => { ctx.renderState.compiling = false; },
    succeeded: () => { ctx.renderState.failure = false; ctx.renderState.failureReason = ""; },
    failed: (reason) => { ctx.renderState.failure = true; ctx.renderState.failureReason = String(reason || "could not render"); },
    resetFailure: () => { ctx.renderState.failure = false; ctx.renderState.failureReason = ""; },
    recordDuration: (seconds) => { if (seconds) ctx.renderState.lastCompile = seconds; },
    recordLatex: (result) => { ctx.renderState.lastLatexResult = result; },
    recordProvenance: (value) => { ctx.renderState.provenance = value; },
    recordDocx: (value) => { ctx.renderState.docxArtifact = value; },
    clearDocx: () => { ctx.renderState.docxArtifact = null; },
  };
  return ctx;
};

// A render, with the page's answers to "what is true now" as a plain object
// the check can move under it. The renderer and the coordinator are the real
// ones; what used to be a VM realm full of free variables is `facts`.
const preview = (values = {}) => {
  const facts = {
    disposed: false,
    navigation: 0,
    source: 0,
    everPainted: false,
    format: "",
    hasSession: true,
    paintsTheFrame: true,
    latexOutput: "pdf",
    typstOutput: "pdf",
    buildPreferences: { selection: "automatic", backend: "auto", output: "html" },
    localExecution: true,
    livePreviewOwnsPane: false,
    ...values.facts,
  };
  const state = {
    compiling: false, lastCompile: 0, failure: false, failureReason: "",
    lastLatexResult: null, provenance: null, docxArtifact: null,
  };
  const status = {
    begin: ({ clearFailure = false } = {}) => {
      state.compiling = true;
      if (clearFailure) state.failure = false;
    },
    finish: () => { state.compiling = false; },
    succeeded: () => { state.failure = false; state.failureReason = ""; },
    failed: (reason) => { state.failure = true; state.failureReason = String(reason || "could not render"); },
    resetFailure: () => { state.failure = false; state.failureReason = ""; },
    recordDuration: (seconds) => { if (seconds) state.lastCompile = seconds; },
    recordLatex: (result) => { state.lastLatexResult = result; },
    recordProvenance: (value) => { state.provenance = value; },
    recordDocx: (value) => { state.docxArtifact = value; },
    clearDocx: () => { state.docxArtifact = null; },
  };
  const harness = {
    facts,
    state,
    published: [],
    diagnostics: [],
    sent: [],
    synctex: [],
    refreshed: 0,
    tree: values.tree || (() => ({ main: "main.md", texts: { "main.md": "x" }, digests: {} })),
  };
  harness.coordinator = createRenderCoordinator({
    navigation: () => facts.navigation,
    source: () => facts.source,
    main: () => harness.tree().main,
    render: (ticket) => harness.render(ticket),
  });
  harness.render = createPreviewRenderer({
    slug: "doc",
    coordinator: harness.coordinator,
    status,
    framePreview: { publish: (payload) => harness.published.push(payload) },
    diagnostics: { rendered: (value) => harness.diagnostics.push(value) },
    renderers: values.renderers,
    snapshotDigest: values.snapshotDigest || (async () => "test-render-digest"),
    diagnosticContext,
    parseSynctex: values.parseSynctex,
    facts: () => facts,
    tree: () => harness.tree(),
    gather: values.gather || (async () => ({ held: { assets: {}, urls: {} }, missing: [] })),
    heading: values.heading || (async () => "Title"),
    takeManual: values.takeManual,
    onsynctex: (parsed) => harness.synctex.push(parsed),
    send: (message) => harness.sent.push(message),
    refreshFrame: () => harness.refreshed++,
  });
  harness.paint = () => harness.coordinator.request();
  return harness;
};

// A compiler result with neither html nor pdf leaves the existing preview up.
{
  const rendered = deferred();
  const harness = preview({
    facts: { format: "latex", everPainted: true },
    tree: () => ({ main: "paper.tex", texts: { "paper.tex": "x" }, digests: {} }),
    snapshotDigest: async () => "digest-A",
    heading: async () => "",
    renderers: {
      formatOf: () => "latex", render: () => rendered.promise,
      producesPdf: () => true,
      failurePage: async () => "<p>failure</p>",
    },
  });
  const pending = harness.paint();
  rendered.resolve({ pdf: null, diagnostics: [{ severity: "error" }] });
  await pending;
  assert.deepEqual(harness.published, [], "the page that was up stays up");
  assert.deepEqual(harness.sent, [], "and nothing replaces it in the frame");
  // The compile is over even though it produced nothing, so the next one is
  // free to start: a failure must not leave the latch held.
  assert.equal(harness.coordinator.running, false);
  assert.equal(harness.state.compiling, false);
}

// HTML is every format's default output, and a render does not ask whether
// the source pane is open before honouring it: somebody who only reads a
// LaTeX document is handed the same flow page as the author typing it.
{
  const asked = [];
  const harness = preview({
    facts: { format: "latex", latexOutput: "html" },
    tree: () => ({ main: "paper.tex", texts: { "paper.tex": "x" }, digests: {} }),
    heading: async () => "",
    renderers: {
      formatOf: () => "latex",
      producesPdf: () => true,
      render: (_tree, _title, options) => {
        asked.push(options.format);
        return Promise.resolve({ html: "<p>page</p>", diagnostics: [] });
      },
    },
  });
  await harness.paint();
  assert.deepEqual(asked, ["html"], "the default output is asked for, editing or not");
  assert.equal(harness.published.at(-1)?.kind, "html", "and a flow page is what reaches the frame");
}

// A live preview owns the pane: this browser's own compile stays out of it.
{
  const harness = preview({
    facts: { format: "quarto", livePreviewOwnsPane: true },
    renderers: { formatOf: () => assert.fail("a live preview must not be compiled over"), producesPdf: () => false },
  });
  await harness.paint();
  assert.deepEqual(harness.published, []);
  assert.equal(harness.refreshed, 0);
}

// A frame showing something this render does not own is refreshed rather than
// painted over.
{
  const harness = preview({
    facts: { paintsTheFrame: false },
    renderers: { formatOf: () => assert.fail("nothing is compiled for a frame this render does not own"), producesPdf: () => false },
  });
  await harness.paint();
  assert.equal(harness.refreshed, 1);
  assert.deepEqual(harness.published, []);
}

console.log("reader-races: all checks passed");

// A nested edit schedules the preview without rebuilding the directory.
{
  let refreshes = 0;
  let changes = 0;
  // Containers are named by id, so the fakes are ids rather than objects. The
  // session says which of them are the directory, exactly as the real one
  // does: the texts, their paths, the figures and the metadata.
  const files = { id: "cid:root-files:Map" };
  const paths = { id: "cid:root-paths:Map" };
  const mainText = { id: "cid:1@1:Text" };
  const directory = new Set([files.id, paths.id, "cid:root-assets:Map", "cid:root-meta:Map"]);
  const active = { files, text: mainText,
    directoryChanged: (events) => !Array.isArray(events) || events.some((event) => directory.has(event.target)) };
  const ctx = context({ session: active, needsSourceRefresh, refreshFiles: () => refreshes++, sourceChanged: () => changes++ });
  vm.runInContext(body("  function filesChanged(events, active = session)", "  function refreshPeers()"), ctx);
  ctx.events = [{ target: "cid:2@1:Text" }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(refreshes, 0, "an edit inside a file does not rebuild the directory");
  assert.equal(changes, 1, "but it does repaint the preview");
  ctx.events = [{ target: mainText.id }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(changes, 1, "the source watcher owns main-text edits");
  ctx.events = [{ target: files.id }];
  vm.runInContext("filesChanged(events)", ctx);
  vm.runInContext("filesChanged({})", ctx);
  assert.equal(refreshes, 2, "a change to the directory rebuilds it");
  assert.equal(changes, 3);
  // One commit is one batch, so a batch that touches both an included file and
  // the directory rebuilds the list once and repaints once.
  // A rename moves a name in `paths` and touches nothing else. The list used
  // to be redrawn only for the texts, so a renamed file kept its old name on
  // screen until the page was reloaded.
  ctx.events = [{ target: paths.id }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(refreshes, 3, "a rename rebuilds the directory");
  assert.equal(changes, 4);
  ctx.events = [{ target: "cid:2@1:Text" }, { target: files.id }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(changes, 5, "one batch schedules one repaint");
  assert.equal(refreshes, 4, "and one rebuild");
  const other = { files: { id: "cid:root-files:Map" }, text: { id: "cid:9@9:Text" }, directoryChanged: () => false };
  ctx.events = [{ target: "cid:2@1:Text" }];
  ctx.other = other;
  vm.runInContext("filesChanged(events, other)", ctx);
  assert.equal(changes, 5, "stale session events are ignored");
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
    // The directory itself belongs to the workspace, which is driven on its
    // own in tests/unit/workspace.mjs. What is checked here is the other half
    // of that read: re-pointing the preview at the file the session now calls
    // its main one, which is a rendering question rather than a directory one.
    files: [], previewFile: "", previewMain: "", sourceFormat: "markdown",
    tick: async () => ({ then: () => {} }), paintPreview: () => {},
    navigationGeneration: 0, everPainted: false, everPaintedShown: false,
    docsOrigin: null, navigateFrame: () => {}, readerDisposed: false,
    framePreview: { clear: () => {} },
    renderCoordinator: { invalidate: () => {} },
    renderStatus: { resetFailure: () => {} },
    renderers: { formatOf: () => "latex", warm: () => {} },
    readQuartoBinding: () => {},
    configureLatex: () => configured++,
  });
  vm.runInContext(body("  function updatePreviewTarget()", "  function previewThisFile()"), ctx);
  vm.runInContext("updatePreviewTarget()", ctx);
  assert.equal(configured, 1);
  assert.equal(ctx.previewMain, "paper.tex");
  assert.equal(ctx.sourceFormat, "latex");
}

// LaTeX configuration follows format/session lifetime. Build settings are
// browser-local and no longer observe collaborative metadata.
{
  const releases = [];
  const configurations = [];
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
  const settings = createBuildSettings({
    slug: "project",
    origin: "https://app.example",
    read: () => ({ selection: "automatic", backend: "auto", engine: "auto" }),
    renderers: { available: () => true },
    paint: () => paints++,
    latex: {
      configure: (value) => configurations.push(value), cancel: () => {}, setSettings: () => {},
      // Compiler releases are mirror-global. Reader setup must not fetch a
      // release list or write an automatic project pin.
      releases: () => { throw new Error("reader must not load compiler releases"); },
    },
  });
  settings.configureLatex("latex", first, "anonymous");
  settings.configureLatex("latex", first, "anonymous");
  assert.equal(configurations.length, 1, "one session is configured once");
  assert.equal(configurations[0].project, "project");
  assert.equal(configurations[0].mayCompile, true);
  assert.equal(first.observers.size, 0);
  assert.equal(paints, 0);
  settings.configureLatex("markdown", first, "anonymous");
  settings.configureLatex("latex", first, "anonymous");
  assert.equal(first.observers.size, 0);
  assert.equal(first.writes.length, 0);
  const second = makeSession();
  settings.configureLatex("latex", second, "anonymous");
  assert.equal(first.observers.size, 0);
  assert.equal(second.observers.size, 0);
  assert.equal(first.writes.length, 0);
  assert.equal(second.writes.length, 0);
  assert.equal(releases.length, 0, "readers must not load or pin a compiler release");
  settings.stopLatex();
  assert.equal(second.observers.size, 0);
  // A cold reader has no edit permission and still compiles from source.
  settings.configureLatex("latex", makeSession(), "anonymous");
  assert.equal(configurations.at(-1).mayCompile, true, "cold readers compile from source without edit permission");
  assert.equal(releases.length, 0, "readers must not pin the default release");
  settings.stopLatex();
}

// A build preference belongs to one reader on one browser. Signing in is a
// different reader, so what the last one had running is interrupted; loading
// the same reader's preference again is not, because source hydration and
// later source changes own the rendering.
{
  const interrupted = [];
  const build = (read) => createBuildSettings({
    slug: "project",
    origin: "https://app.example",
    read,
    renderers: { available: () => true },
    latex: { configure: () => {}, cancel: () => {}, setSettings: () => {} },
    interrupt: (next) => interrupted.push(next),
  });
  const settings = build((scope, format) =>
    ({ selection: "tool", backend: "local", tool: "calepin", output: "pdf", user: scope.user, format }));
  settings.follow({ format: "typst", user: "anonymous" });
  assert.deepEqual(interrupted, [], "the first load interrupts nothing");
  settings.follow({ format: "typst", user: "anonymous" });
  assert.deepEqual(interrupted, [], "the same reader's preference, again, interrupts nothing");
  settings.follow({ format: "typst", user: "google:ada" });
  assert.deepEqual(interrupted, [null], "signing in is a different reader");
  assert.equal(settings.state.preferences.user, "google:ada", "and their preference is the one loaded");

  // A preference chosen by hand interrupts with the choice itself, so a local
  // preview that the choice keeps using can survive it.
  settings.choose({ selection: "tool", backend: "local", tool: "calepin" }, "typst");
  assert.deepEqual(interrupted.at(-1), { selection: "tool", backend: "local", tool: "calepin" });
}

// What a preference says about the outputs and the preview modes, and the one
// way the two paths differ: a preference *loaded* for a format speaks for that
// format whether or not the document is in it -- the Typst mode is read when a
// Typst file is opened later -- while one *chosen* by hand speaks only for the
// document the reader was looking at when they chose it.
{
  const calepin = { selection: "tool", backend: "local", tool: "calepin", output: "pdf" };
  const settings = createBuildSettings({
    slug: "project", origin: "https://app.example",
    read: () => calepin,
    renderers: { available: () => true },
    latex: { configure: () => {}, cancel: () => {}, setSettings: () => {} },
  });

  // Loading a Typst preference while a Markdown document is on screen still
  // says what Typst will do.
  settings.follow({ format: "markdown", user: "anonymous" });
  assert.equal(settings.state.typstPreviewMode, "calepin", "a loaded preference speaks for its format");
  assert.equal(settings.state.quartoPreviewMode, "markdown", "and for Quarto, when a tool was chosen");

  // Choosing one while a Markdown document is on screen says nothing about
  // Typst, because that is not what the reader was looking at.
  settings.state.typstPreviewMode = "typst";
  settings.choose(calepin, "markdown");
  assert.equal(settings.state.typstPreviewMode, "typst", "a chosen preference speaks only for the document on screen");
  settings.choose(calepin, "typst");
  assert.equal(settings.state.typstPreviewMode, "calepin");

  // The output each format produces follows the preference it was loaded or
  // chosen with, and anything that is not a PDF is a flow page.
  settings.choose({ ...calepin, output: "html" }, "typst");
  assert.equal(settings.state.typstOutput, "html");
  settings.choose({ ...calepin, output: "pdf" }, "typst");
  assert.equal(settings.state.typstOutput, "pdf");
  settings.choose({ ...calepin, output: "docx" }, "typst");
  assert.equal(settings.state.typstOutput, "html", "an output that is not paged falls back to the default flow page");
  settings.choose({ selection: "tool", backend: "browser", tool: "tex", output: "html" }, "latex");
  assert.equal(settings.state.latexOutput, "html");
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
  const harness = preview({
    facts: { format: "markdown", everPainted: true },
    tree: () => ({ main, texts: { [main]: text }, digests: {} }),
    renderers: {
      formatOf: () => "markdown",
      producesPdf: () => false,
      render: (tree) => {
        calls.push(tree);
        return calls.length === 1 ? first.promise : second.promise;
      },
    },
  });
  const pending = harness.paint();
  await Promise.resolve();
  text = "intermediate";
  harness.facts.source++;
  await harness.paint();
  text = "latest";
  harness.facts.source++;
  if (invalidate === "navigation") harness.facts.navigation++;
  if (invalidate === "main") main = "other.md";
  await harness.paint();
  assert.equal(calls.length, 1, "only one compile can be in flight");
  first.resolve({ html: "first", diagnostics: [] });
  await pending;
  await new Promise(setImmediate);
  delivered.push(...harness.published.map((payload) => payload.html));
  diagnostics.push(...harness.diagnostics);
  assert.deepEqual(delivered, invalidate ? [] : ["first"]);
  assert.equal(diagnostics.length, 0, "outdated diagnostics stay hidden");
  assert.equal(calls.length, 2);
  assert.equal(calls[1].texts[main], "latest");
  second.resolve({ html: "latest", diagnostics: [] });
  await new Promise(setImmediate);
  assert.equal(harness.published.at(-1).html, "latest");
  assert.equal(harness.coordinator.running, false);
}

// A peer's edit advances the source generation the history panel reads, so a
// comparison against "current" knows it was taken before that edit.
//
// It also recomputes the review. A hunk's offsets are a claim about the
// proposal's base, so an edit underneath one does not move it -- it makes it
// a claim about text that is no longer there, which is the difference between
// a card marked stale and a card still offering to replace words nobody is
// looking at.
{
  let reviewed = 0;
  let rescheduled = 0;
  const ctx = context({
    sourceGeneration: 0, historyLiveVersion: 0, mayEdit: true, publishedBundle: null,
    outlineRevision: 0, sourceFormat: "", editing: false,
    previewTimer: null, PASSIVE_PREVIEW_DEBOUNCE: 1000,
    setTimeout: () => 1, clearTimeout: () => {}, paintPreview: () => {},
    diagnosticPainter: { typed: () => {} },
    refreshReview: () => { reviewed += 1; },
    bundle: { keepCurrent: () => { rescheduled += 1; } },
  });
  vm.runInContext(body("  function sourceChanged()", "  /* ------------------------------------------------------- keeping in step */"), ctx);
  vm.runInContext("sourceChanged()", ctx);
  assert.equal(ctx.sourceGeneration, 1);
  assert.equal(ctx.historyLiveVersion, 1);
  assert.equal(reviewed, 1, "an edit under a proposal recomputes its hunks");
  assert.equal(rescheduled, 0, "an edit never schedules a stored reader rendition");
}

// Further keystrokes must not postpone an editor's already scheduled preview.
{
  let scheduled = 0;
  const ctx = context({
    sourceGeneration: 0, historyLiveVersion: 0, editing: true, sourceFormat: "markdown", pdfOutput: false, mayEdit: true, publishedBundle: null,
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
    sourceGeneration: 0, historyLiveVersion: 0, editing: true, sourceFormat: "typst", pdfOutput: true, compilesHere: true,
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

// Loro may notify the main-text observer from inside CodeMirror's update
// listener. Diagnostic painting must wait until that update has returned;
// otherwise Editor.setDiagnostics dispatches a forbidden nested update.
{
  let inEditorUpdate = true;
  let paintedDiagnostics = 0;
  const ctx = context({
    sourceGeneration: 0, historyLiveVersion: 0, quartoFreshnessSerial: 0, sourceFormat: "quarto", quartoView: "draft",
    session: {
      mainPath: () => "main.qmd", mainId: () => "main", textOf: () => ({ toString: () => "# Draft\n" }),
      text: { toString: () => "# Draft\n" },
    },
    quarto: { parseQuarto: () => ({ diagnostics: [], source: "# Draft\n" }) },
    diagnosticsController: { render: () => {
      if (inEditorUpdate) throw new Error("nested CodeMirror update");
      paintedDiagnostics++;
    } },
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
  const builds = [];
  const harness = preview({
    facts: { format: "quarto", everPainted: true },
    tree: () => ({ main: "main.qmd", texts: { "main.qmd": "source" }, digests: {} }),
    renderers: {
      formatOf: () => "quarto", producesPdf: () => false,
      render: async (_tree, _title, options) => {
        builds.push(options.buildPreferences);
        return { html: "<p>draft</p>", diagnostics: [] };
      },
    },
  });
  await harness.paint();
  assert.equal(builds.length, 1, "a Quarto document with no live preview compiles its own draft");
  assert.equal(harness.published[0].kind, "html");
  assert.equal(harness.published[0].html, "<p>draft</p>");

  // And a reader who has not approved execution gets the Markdown it is,
  // rather than code this browser was never given permission to run.
  harness.facts.localExecution = false;
  await harness.paint();
  assert.deepEqual(builds.at(-1), { selection: "tool", backend: "browser", tool: "markdown", output: "html" });
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
  const harness = preview({
    facts: { format: "typst" },
    tree: () => ({ main: "main.typ", texts: { "main.typ": text }, digests: {} }),
    snapshotDigest: async () => "digest-typst",
    renderers: {
      formatOf: () => "typst",
      producesPdf: () => true,
      render: (tree) => {
        calls.push(tree);
        return calls.length === 1 ? first.promise : second.promise;
      },
    },
  });
  const pending = harness.paint();
  await new Promise(setImmediate);
  text = "latest";
  harness.facts.source++;
  await harness.paint();
  assert.equal(calls.length, 1, "Typst keeps one PDF compile active");
  first.resolve({ pdf: Uint8Array.of(1), diagnostics: [] });
  await pending;
  await new Promise(setImmediate);
  assert.equal(calls.length, 2, "Typst queues the latest source snapshot");
  second.resolve({ pdf: Uint8Array.of(2), diagnostics: [] });
  await new Promise(setImmediate);
  delivered.push(...harness.published.map((payload) => payload.bytes[0]));
  diagnostics.push(...harness.diagnostics);
  held.push(...harness.sent);
  assert.deepEqual(delivered, [1, 2], "Typst may show an intermediate PDF, then the latest one");
  assert.equal(held.length, 0, "Typst output is transient and is never retained");
  assert.equal(diagnostics.length, 1, "an intermediate Typst PDF cannot clear newer diagnostics");
}

// A first Typst compile that returns structured diagnostics must leave the
// reader in its explicit PDF failure state instead of a blank frame.
{
  const renderers = {
    formatOf: () => "typst", producesPdf: () => true,
    render: async () => ({ pdf: null, diagnostics: [{ severity: "error", message: "broken" }] }),
    failurePage: async () => null,
  };
  let tree = () => ({ main: "main.typ", texts: { "main.typ": "bad" }, digests: {} });
  const harness = preview({
    facts: { format: "typst" },
    tree: () => tree(),
    snapshotDigest: async () => "digest-typst",
    renderers,
  });
  harness.tree = () => tree();
  await harness.paint();
  assert.equal(harness.state.failure, true, "structured Typst diagnostics set the PDF failure state");
  assert.deepEqual(
    harness.published, [],
    "a failed first Typst compile leaves no blank success preview",
  );

  // A compiler setup failure has neither a TeX log nor source diagnostics.
  // Its structured reason must reach the failure pane.
  harness.facts.format = "latex";
  tree = () => ({ main: "main.tex", texts: { "main.tex": "source" }, digests: {} });
  renderers.formatOf = () => "latex";
  renderers.render = async () => ({ pdf: null, log: "", diagnostics: [], failure: { message: "The mirror has no engine release" } });
  await harness.paint();
  assert.equal(harness.state.failureReason, "The mirror has no engine release");

  // HTML conversion errors belong to the source snapshot that caused them.
  const pendingHtml = deferred();
  harness.facts.latexOutput = "html";
  harness.state.failure = false;
  harness.state.failureReason = "";
  renderers.render = (_tree, _title, options) => {
    assert.equal(options.format, "html");
    return pendingHtml.promise;
  };
  const htmlPaint = harness.paint();
  await new Promise(setImmediate);
  harness.facts.source++;
  pendingHtml.reject(new Error("old conversion failed"));
  await htmlPaint;
  assert.equal(harness.state.failure, false, "an old HTML error cannot mark a newer edit failed");
  assert.equal(harness.state.failureReason, "");
}
console.log("reader-races: continuous preview, render coalescing and navigation guards passed");

// Ctrl-S names where the work is, and reserves "saved" for the server. This
// browser's own storage is evictable and is one device, so an offline Ctrl-S
// says what will happen to the typing rather than calling it saved.
{
  const said = [];
  const ctx = context({
    connected: true, persistence: { pending: 1, local: true },
    say: (value) => { said.push(value); },
  });
  vm.runInContext(body("  function reportPersistence()", "  // There is no save, so a close"), ctx);
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, [], "unacknowledged writes are not reported as anything");
  ctx.persistence.pending = 0;
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, ["Saved on the server."]);
  ctx.connected = false;
  vm.runInContext("reportPersistence()", ctx);
  assert.match(said[1], /^Offline. Everything typed so far has already reached the server./);
  ctx.persistence.pending = 2;
  vm.runInContext("reportPersistence()", ctx);
  assert.match(said[2], /in this browser only/, "work the server has not seen is not called saved");
  assert.doesNotMatch(said[1] + said[2], /saved/i, "the word is the server's");
  // A local write that failed says so through its own failure, not here.
  ctx.persistence = { pending: 0, local: true, localError: "quota exceeded" };
  vm.runInContext("reportPersistence()", ctx);
  assert.equal(said.length, 3);
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
  active.joinOptions.send({ type: "doc-update", update: "held" });
  assert.equal(sent.length, 1, "the initial y-open is sent");
  rooms[0].onConnected(false);
  const stale = rooms[0].onConnected(true);
  const current = rooms[0].onConnected(true);
  second.resolve({ ok: true, json: async () => ({ created_at: "first", role: "editor" }) });
  await current;
  first.resolve({ ok: true, json: async () => ({ created_at: "first", role: "editor" }) });
  await stale;
  assert.equal(sent.filter((message) => message.type === "doc-open").length, 2, "only the current reconnect re-opens");
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
  assert.equal(sent.filter((message) => message.type === "doc-open").length, 1);
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
  vm.runInContext(`${localAppHelpers}\n${body('  function updatePreviewTarget()', '  // A file added')}`, ctx);
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

// Main and secondary previews never expose the retired rendering store.
assert.doesNotMatch(reader, /createRenderingStore|holdRendering|\/renderings\//);

// Desktop View and the compact menu dispatch the same preview commands.
{
  const commands = [];
  const ctx = context({
    previewThisFile: () => commands.push('file'),
    toggleLocalExecution: () => commands.push('local-execution'),
    setLatexOutput: mode => commands.push(`latex-${mode}`),
    chose: value => commands.push(value),
    FILE_COMMANDS: [],
  });
  vm.runInContext(body('  function chooseViewCommand(value)', '  // The File menu.'), ctx);
  vm.runInContext(body('  function chooseCompactCommand(value)', '  /* ------------------------------------------------------------------- boot */'), ctx);
  vm.runInContext('chooseViewCommand("preview-file"); chooseViewCommand("local-execution"); chooseCompactCommand("local-execution"); chooseViewCommand("layout-split")', ctx);
  assert.deepEqual(commands, ['file','local-execution','local-execution','layout-split']);
  vm.runInContext('chooseViewCommand("preview-latex-html"); chooseCompactCommand("preview-latex-pdf")', ctx);
  assert.deepEqual(commands.slice(-2), ['latex-html', 'latex-pdf']);
  const view = body('{#snippet viewItems()}', '<Nav {me}>');
  const file = body('{#snippet fileItems()}', '{#snippet layoutItems()}');
  assert.match(view, /Preview this file/);
  assert.match(view, /@render previewItems\(\)/);
  // Local execution is its own section of the View menu, not another engine.
  assert.match(view, /menu-section-label">Local execution<\/div>/);
  assert.match(view, /value="local-execution"/);
  assert.doesNotMatch(reader, /preview-quarto|preview-calepin|preview-markdown|preview-typst"/);
  assert.doesNotMatch(file, /preview-/);
  // Settings and Compile now live under File; there is no Tools menu left.
  assert.match(file, /value="settings"/);
  assert.match(file, /value="compile"/);
  assert.doesNotMatch(reader, /label="Tools"/);
}
console.log('reader-races: View and compact menus expose the same explicit preview controls');

// Restoring replaces every file in the project, so what it discards has to be
// what the reader was shown. A write that lands while the dialog is open --
// a collaborator's edit, or this reader in another tab -- must be confirmed
// again rather than quietly thrown away.
{
  // Reader reads the signature and hands it over, so the check drives the
  // timeline the same way: what a confirmation is worth depends on the
  // project it was given, not on what a session says a moment later.
  const posted = [];
  const timeline = createTimeline({
    slug: "doc",
    key: "",
    history: { loadWithStatus: async () => ({ checkpoints: [], durability: null }) },
    fetcher: async (url, options) => {
      posted.push({ url, body: options.body });
      return { ok: true, json: async () => ({}) };
    },
    problem: (message) => assert.fail(message),
  });
  timeline.state.canEdit = true;
  const asShown = JSON.stringify({ main: "main.md", texts: { "main.md": "as confirmed" }, files: {} });
  const moved = JSON.stringify({ main: "main.md", texts: { "main.md": "somebody else typed" }, files: {} });

  timeline.ask("sha-one", asShown);
  assert.equal(timeline.state.restoring, true);
  await timeline.confirm(moved);
  assert.equal(posted.length, 0, "a project that moved is not restored over without a second look");
  assert.equal(timeline.state.restoreMoved, true, "and the dialog says so");
  assert.equal(timeline.state.restoring, true, "and stays open");

  await timeline.confirm(moved);
  assert.equal(posted.length, 1, "confirming against what is now on the screen restores");
  assert.equal(JSON.parse(posted[0].body).sha, "sha-one");
  assert.equal(timeline.state.restoring, false);
  assert.equal(timeline.state.restoreMoved, false);

  // A reader who cannot edit cannot restore, whatever reaches the action.
  const readOnly = createTimeline({ slug: "doc", fetcher: () => assert.fail("a reader must not restore") });
  readOnly.ask("sha-one", asShown);
  assert.equal(readOnly.state.restoring, false);
  await readOnly.confirm(asShown);
}
console.log('reader-races: a restore is confirmed against the project it will replace');
