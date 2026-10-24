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
import { createProjectSession } from "../../src/lib/project-session.js";
import { createPreviewRenderer } from "../../src/lib/reader/preview-render.js";
import { createRenderCoordinator } from "../../src/lib/reader/render-coordinator.js";
import { loadRunes } from "../helpers/runes.mjs";
import { needsSourceRefresh } from "../../src/lib/reader/source-events.js";
import { openingFile, previewTarget, rendersAlone, includersOf } from "../../src/lib/opening.js";

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

// Initial Quarto setup must configure the desktop app as active for an editor.
// If permission is assigned afterwards, editable onboarding examples remain
// on the Markdown fallback even when the desktop app is already there.
{
  const prepare = body("  async function prepare(document_)", "  $effect(() => {");
  assert.ok(
    prepare.indexOf("mayEdit = Boolean(allowed)") < prepare.indexOf("scopeLocalApp()"),
    "prepare resolves edit permission before scoping the local app, which is active only for an editor",
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

// A failed metadata request can open this browser's local copy of a project;
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
  assert.equal(documents[0].local_copy, true);
  assert.deepEqual(errors, []);
}

// HTTP errors carry a status property; network errors do not.
// This allows the reader to distinguish a missing document (404) from
// a server error (5xx) or network failure.
{
  const errors = [];
  const collectError = (error) => errors.push({ message: error.message, status: error.status });

  // A 503 Service Unavailable error carries the status code.
  const boot503 = createReaderBoot({
    slug: "unavailable",
    fetcher: async () => ({ ok: false, status: 503 }),
    whoami: async () => ({}),
    findLocal: async () => { throw new Error("not in cache"); },
    onError: collectError,
  });
  await boot503.start();
  assert.equal(errors.length, 1);
  assert.equal(errors[0].status, 503);

  // An authorization refusal must reach the reader's sign-in state even if
  // this browser has an older prepared copy; the cache is only for outages.
  errors.length = 0;
  let lookedInCache = false;
  const boot401 = createReaderBoot({
    slug: "private",
    fetcher: async () => ({ ok: false, status: 401 }),
    whoami: async () => ({}),
    findLocal: async () => { lookedInCache = true; return { document: { title: "Cached paper" } }; },
    onError: collectError,
  });
  await boot401.start();
  assert.equal(lookedInCache, false);
  assert.deepEqual(errors, [{ message: "not found", status: 401 }]);

  // A network error (thrown fetch) has no status property.
  errors.length = 0;
  const bootNetwork = createReaderBoot({
    slug: "unreachable",
    fetcher: async () => { throw new Error("network timeout"); },
    whoami: async () => ({}),
    findLocal: async () => { throw new Error("not in cache"); },
    onError: collectError,
  });
  await bootNetwork.start();
  assert.equal(errors.length, 1);
  assert.equal(errors[0].status, undefined);
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
    // Recomputing the changes is part of what an edit does now; the blocks
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
    previewPath: "",
    previewReason: "none",
    previewIncluders: [],
    lockedFile: "",
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
    compiling: false, failure: false, failureReason: "",
    lastLatexResult: null, docxArtifact: null,
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
    recordLatex: (result) => { ctx.renderState.lastLatexResult = result; },
    recordLog: (log) => { ctx.renderState.lastLog = log || ""; },
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
    compiling: false, failure: false, failureReason: "",
    lastLatexResult: null, docxArtifact: null,
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
    recordLatex: (result) => { state.lastLatexResult = result; },
    recordLog: (log) => { state.lastLog = log || ""; },
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
      formatOf: () => "latex", previewFormatOf: () => "latex", render: () => rendered.promise,
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
      previewFormatOf: () => "latex",
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
    renderers: { formatOf: () => assert.fail("a live preview must not be compiled over"), previewFormatOf: () => assert.fail("a live preview must not be compiled over"), producesPdf: () => false },
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
    renderers: { formatOf: () => assert.fail("nothing is compiled for a frame this render does not own"), previewFormatOf: () => assert.fail("nothing is compiled for a frame this render does not own"), producesPdf: () => false },
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
  const openText = { id: "cid:1@1:Text" };
  const directory = new Set([files.id, paths.id, "cid:root-assets:Map", "cid:root-meta:Map"]);
  const active = { files,
    directoryChanged: (events) => !Array.isArray(events) || events.some((event) => directory.has(event.target)) };
  let targets = 0;
  const ctx = context({ session: active, needsSourceRefresh, refreshFiles: () => refreshes++, sourceChanged: () => changes++, scheduleTargetUpdate: () => targets++ });
  vm.runInContext(body("  function filesChanged(events, active = session)", "  function refreshPeers()"), ctx);
  ctx.events = [{ target: "cid:2@1:Text" }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(refreshes, 0, "an edit inside a file does not rebuild the directory");
  assert.equal(changes, 1, "but it does repaint the preview");
  assert.equal(targets, 1, "and it asks for the preview target to be worked out again");
  ctx.events = [{ target: openText.id }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(changes, 2, "an edit to the open file is heard here too, since no watcher is bound to it");
  ctx.events = [{ target: files.id }];
  vm.runInContext("filesChanged(events)", ctx);
  vm.runInContext("filesChanged({})", ctx);
  assert.equal(refreshes, 2, "a change to the directory rebuilds it");
  assert.equal(changes, 4);
  // One commit is one batch, so a batch that touches both an included file and
  // the directory rebuilds the list once and repaints once.
  // A rename moves a name in `paths` and touches nothing else. The list used
  // to be redrawn only for the texts, so a renamed file kept its old name on
  // screen until the page was reloaded.
  ctx.events = [{ target: paths.id }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(refreshes, 3, "a rename rebuilds the directory");
  assert.equal(changes, 5);
  ctx.events = [{ target: "cid:2@1:Text" }, { target: files.id }];
  vm.runInContext("filesChanged(events)", ctx);
  assert.equal(changes, 6, "one batch schedules one repaint");
  assert.equal(refreshes, 4, "and one rebuild");
  const other = { files: { id: "cid:root-files:Map" }, text: { id: "cid:9@9:Text" }, directoryChanged: () => false };
  ctx.events = [{ target: "cid:2@1:Text" }];
  ctx.other = other;
  vm.runInContext("filesChanged(events, other)", ctx);
  assert.equal(changes, 6, "stale session events are ignored");
}

// Pointing the preview at a LaTeX file configures the already joined project,
// even though the initial prepare ran while it was Markdown.
{
  let configured = 0;
  const ctx = context({
    session: { latexSettings: () => ({ engine: "auto" }) },
    // The directory itself belongs to the workspace, which is driven on its
    // own in tests/unit/workspace.mjs. What is checked here is the other half
    // of that read: re-pointing the preview at the file that is open, which is
    // a rendering question rather than a directory one.
    files: [], sourceFormat: "markdown",
    openTextPath: "paper.tex", lockedPath: "", aloneSet: new Set(), visitorFile: "",
    liveTreeNow: () => ({ texts: { "paper.tex": "\\documentclass{article}" }, digests: {} }),
    previewTarget, rendersAlone, openingFile, includersOf, readerProjectOwner: {},
    restoreLastPreview: () => {},
    tick: async () => ({ then: () => {} }), paintPreview: () => {},
    navigationGeneration: 0, everPainted: false, everPaintedShown: false,
    docsOrigin: null, navigateFrame: () => {},
    framePreview: { clear: () => {} },
    renderCoordinator: { invalidate: () => {} },
    renderStatus: { resetFailure: () => {} },
    renderers: { formatOf: () => "latex", previewFormatOf: () => "latex", warm: () => {} },
    readQuartoBinding: () => {},
    configureLatex: () => configured++,
  });
  vm.runInContext(body("  function updatePreviewTarget()", "  // The preview follows the open file"), ctx);
  vm.runInContext("updatePreviewTarget()", ctx);
  assert.equal(configured, 1);
  assert.equal(ctx.previewPath, "paper.tex");
  assert.equal(ctx.previewReason, "alone");
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

// What a preference says about the outputs and the preview modes. A loaded
// preference never puts a tool that runs the document in charge: Quarto and
// Calepin follow the per-document permission, which is chosen by hand, and a
// chosen preference speaks only for the document on screen.
{
  const calepin = { selection: "tool", backend: "local", tool: "calepin", output: "pdf" };
  const settings = createBuildSettings({
    slug: "project", origin: "https://app.example",
    read: () => calepin,
    latex: { configure: () => {}, cancel: () => {}, setSettings: () => {} },
  });

  // A remembered local preference, loaded for either format, leaves both on
  // the browser's own renderer.
  for (const format of ["markdown", "typst", "quarto"]) {
    settings.follow({ format, user: "anonymous" });
    assert.equal(settings.state.typstPreviewMode, "typst", "a loaded preference never starts Calepin");
    assert.equal(settings.state.quartoPreviewMode, "markdown", "a loaded preference never starts Quarto");
  }

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
      previewFormatOf: () => "markdown",
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
// It also refreshes the rows of the Changes panel, because an edit can move
// or remove the change it lands in.
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
  assert.equal(reviewed, 1, "an edit refreshes the rows of the Changes panel");
  assert.equal(rescheduled, 0, "an edit never schedules a stored reader rendition");
}

// The Changes rows come from the document's own pending changes, with the
// file path and the starting line from the session, and a selection that no
// longer exists is let go.
{
  const listed = [
    { id: "a", file: "f1", who: "acct-1", author: "Ada", run: null, at: 5, from: 4, to: 9, deleted: "cat", inserted: "tabby" },
    { id: "b", file: "f2", who: "agent:x", author: "X", run: "run-1", at: 6, from: 0, to: 0, deleted: "gone", inserted: "" },
  ];
  const ctx = context({
    session: { doc: {}, paths: new Map([["f1", "paper.md"]]), textOf: (id) => (id === "f1" ? "the\ncat sat" : null) },
    changeRows: [], selectedChange: "old",
    listChanges: () => listed,
  });
  vm.runInContext(body("  function lineAt(", "  // Recomputing reads every change"), ctx);
  vm.runInContext("recomputeChanges()", ctx);
  assert.deepEqual(JSON.parse(JSON.stringify(ctx.changeRows)), [
    { id: "a", file_id: "f1", path: "paper.md", author: "Ada", who: "acct-1", run: "", at: 5, position: 4, line: 2, before: "cat", after: "tabby" },
    { id: "b", file_id: "f2", path: "", author: "X", who: "agent:x", run: "run-1", at: 6, position: 0, before: "gone", after: "" },
  ], "each pending change becomes one row");
  assert.equal(ctx.selectedChange, null, "a selected change that is gone is let go");
  ctx.selectedChange = "b";
  vm.runInContext("recomputeChanges()", ctx);
  assert.equal(ctx.selectedChange, "b", "a selected change that is still pending stays selected");
  ctx.session = null;
  vm.runInContext("recomputeChanges()", ctx);
  assert.deepEqual(JSON.parse(JSON.stringify(ctx.changeRows)), [], "no session, no rows");
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
    previewPath: "main.qmd",
    session: {
      idOf: () => "main", textOf: () => ({ toString: () => "# Draft\n" }),
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
      formatOf: () => "quarto", previewFormatOf: () => "quarto", producesPdf: () => false,
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
      previewFormatOf: () => "typst",
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
    formatOf: () => "typst", previewFormatOf: () => "typst", producesPdf: () => true,
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
  renderers.previewFormatOf = () => "latex";
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
    connected: true, persistence: { pending: 1, local: true, joined: true },
    say: (value) => { said.push(value); },
  });
  vm.runInContext(body("  function reportPersistence()", "  // There is no save, so a close"), ctx);
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, [], "work without durable coverage is not reported as saved");
  ctx.persistence.pending = 0;
  ctx.persistence.joined = false;
  vm.runInContext("reportPersistence()", ctx);
  assert.deepEqual(said, [], "a connected socket is not a completed join");
  ctx.persistence.joined = true;
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

// Late notifications cannot reach a disposed reader. Two halves: the session
// used here is the real one, with a persistence adapter whose write lands
// after teardown; and a deliberately unguarded fake, which is what makes this
// a check of the collaboration owner rather than of the session's own guard.
{
  const states = [];
  let handlers = null;
  let roomOptions;
  const collaboration = createReaderCollaboration({
    slug: "example",
    fetcher: async () => ({ ok: true, json: async () => ({ created_at: "first", role: "editor" }) }),
    openRoom: (_slug, options) => {
      roomOptions = options;
      return { send: () => {}, sendLive: () => ({ ok: true }), close: () => {} };
    },
    collab: {
      join: (options) => createProjectSession({
        ...options,
        presenceId: () => "late-write",
        persistence: {
          open(_doc, events) {
            handlers = events;
            return { close() {} };
          },
        },
      }),
    },
    getCanEdit: () => true,
    onState: (state) => states.push(state),
  });
  collaboration.start({ created_at: "first", role: "editor" });
  handlers.hydrated();
  assert.ok(states.length > 0, "a live session reports its persistence state");
  const before = states.length;
  collaboration.close();
  // The adapter was closed, but a write it had already begun still completes.
  handlers.writing(1);
  handlers.persisted();
  handlers.failed(new Error("quota full"));
  assert.equal(states.length, before, "a closed collaboration delivers no persistence state");
  assert.equal(roomOptions.onConnected !== undefined, true);
}
{
  const events = [];
  const fake = fakeCollaboration();
  const collaboration = createReaderCollaboration({
    slug: "example",
    fetcher: async () => ({ ok: true, json: async () => ({ created_at: "first", role: "editor" }) }),
    openRoom: () => ({ send: () => {}, sendLive: () => ({ ok: true }), close: () => {} }),
    collab: fake.module,
    getCanEdit: () => true,
    onState: () => events.push("state"),
    onPeers: () => events.push("peers"),
    onAwareness: () => events.push("awareness"),
  });
  collaboration.start({ created_at: "first", role: "editor" });
  const options = fake.session.joinOptions;
  options.onState({ pending: 0 });
  options.onPeers(2);
  assert.deepEqual(events, ["state", "peers"]);
  collaboration.close();
  options.onState({ pending: 1 });
  options.onPeers(3);
  assert.deepEqual(events, ["state", "peers"], "a session that outlives its owner updates nothing");
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

// The preview follows the file open in the source. A file that cannot render
// alone is shown inside the one file that includes it, a file with nothing to
// draw empties the preview, and a lock keeps the preview on one file while the
// source moves on.
{
  const texts = {
    'thesis.tex': '\\documentclass{book}\n\\input{chapters/intro}',
    'chapters/intro.tex': 'Introduction',
    'appendix.tex': '\\documentclass{article}',
    'refs.bib': '@book{a}',
  };
  const files = Object.keys(texts).map(path => ({ id: path, path, kind: 'text' }));
  files.push({ id: 'logo.png', path: 'logo.png', kind: 'asset' });
  const configured = [];
  let cleared = 0, painted = 0;
  const ctx = context({
    session: { paths: new Map(files.map(f => [f.id, f.path])) },
    files, openFile: 'thesis.tex', sourceFormat: '', aloneSet: new Set(), visitorFile: '',
    compact: false, layout: 'source', showMobileView: () => {},
    viewing: null, editing: true, editor: { text: () => null },
    liveTreeNow: () => ({ texts, digests: {} }),
    previewTarget, rendersAlone, openingFile, includersOf, readerProjectOwner: {},
    restoreLastPreview: () => {},
    renderers: { formatOf: p => p.endsWith('.tex') ? 'latex' : '', previewFormatOf: p => p.endsWith('.tex') ? 'latex' : '', warm: () => {} },
    configureLatex: f => configured.push(f),
    localQuarto: { configure: () => {}, bindingId: () => 'hosted', probe: async () => {} },
    SLUG: 'thesis', location: {origin:'http://localhost'},
    navigationGeneration: 0, issued: 0, docsOrigin: null,
    framePreview: {clear: () => cleared++},
    renderCoordinator: { invalidate: () => {} },
    renderStatus: { resetFailure: () => {}, clearDocx: () => {} },
    diagnosticPainter: { rendered: () => {} },
    previewTimer: null, deliveredKind: '', activeSynctex: null, previewCleared: false,
    navigateFrame: () => {}, readQuartoBinding: () => {},
    tick: async () => {}, paintPreview: () => painted++,
  });
  // `lockedPath`, `openTextPath` and the visitor-aware `filesShown` and
  // `shownOpenId` are derived in the page; here they are read from the same
  // two ids, as an editor with a session sees them.
  vm.runInContext(`
    Object.defineProperty(globalThis, 'filesShown', { get() { return files; } });
    Object.defineProperty(globalThis, 'shownOpenId', { get() { return openFile; } });
    Object.defineProperty(globalThis, 'lockedPath', { get() { return files.find((f) => f.id === lockedFile)?.path || ''; } });
    Object.defineProperty(globalThis, 'openTextPath', { get() { return files.find((f) => f.id === openFile && f.kind === 'text')?.path || ''; } });
  `, ctx);
  vm.runInContext(`${body('  /// Empties the preview', '  // A project with no file at all')}\n${body('  function updatePreviewTarget()', '  // The preview follows the open file')}\n${body('  function lockPreviewOn(id)', '  // Any edit can change')}`, ctx);
  vm.runInContext(body('  function treeNow()', '  // Painting the preview'), ctx);
  const open = (id) => { ctx.openFile = id; vm.runInContext('updatePreviewTarget()', ctx); };

  vm.runInContext('updatePreviewTarget()', ctx);
  assert.equal(ctx.previewPath, 'thesis.tex', 'the preview starts on the open file');
  assert.equal(ctx.previewReason, 'alone');
  assert.deepEqual([...ctx.aloneSet].sort(), ['appendix.tex', 'thesis.tex'], 'the files that render alone are the ones the eye offers');

  open('appendix.tex');
  assert.equal(ctx.previewPath, 'appendix.tex', 'opening a file that renders alone moves the preview');
  assert.equal(vm.runInContext('treeNow().main', ctx), 'appendix.tex');
  await new Promise(setImmediate);
  assert.equal(painted, 1);

  open('chapters/intro.tex');
  assert.equal(ctx.previewPath, 'thesis.tex', 'an included file is shown inside its includer');
  assert.equal(ctx.previewReason, 'inside');
  assert.equal(vm.runInContext('treeNow().main', ctx), 'thesis.tex');

  open('refs.bib');
  assert.equal(ctx.previewPath, '', 'a file with nothing to draw empties the preview');
  assert.equal(ctx.previewReason, 'none');
  assert.equal(ctx.sourceFormat, '');

  // The eye locks without moving the source.
  vm.runInContext('lockPreview("appendix.tex")', ctx);
  assert.equal(ctx.openFile, 'refs.bib', 'locking does not open the file');
  assert.equal(ctx.previewPath, 'appendix.tex');
  assert.equal(ctx.previewReason, 'locked');
  assert.equal(ctx.layout, 'split', 'a source-only layout makes room for the preview');
  open('chapters/intro.tex');
  assert.equal(ctx.previewPath, 'appendix.tex', 'the lock holds while the source moves');
  assert.equal(vm.runInContext('treeNow().main', ctx), 'appendix.tex');

  vm.runInContext('lockPreview("chapters/intro.tex")', ctx);
  assert.equal(ctx.lockedFile, 'appendix.tex', 'a file that cannot render alone cannot be locked');

  // A figure open in the source leaves the preview where it is.
  open('logo.png');
  assert.equal(ctx.previewPath, 'appendix.tex');

  vm.runInContext('unlockPreview()', ctx);
  open('chapters/intro.tex');
  assert.equal(ctx.previewPath, 'thesis.tex', 'unlocking returns to following the open file');
  assert.equal(ctx.lockedFile, '');

  // The lock is by identity: a rename keeps it, and a delete drops it.
  vm.runInContext('lockPreview("appendix.tex")', ctx);
  files.find(f => f.id === 'appendix.tex').path = 'annex.tex';
  texts['annex.tex'] = texts['appendix.tex']; delete texts['appendix.tex'];
  open('chapters/intro.tex');
  assert.equal(ctx.previewPath, 'annex.tex');
  ctx.files = files.filter(f => f.id !== 'appendix.tex');
  delete texts['annex.tex'];
  open('chapters/intro.tex');
  assert.equal(ctx.lockedFile, '', 'deleting the locked file drops the lock');
  assert.equal(ctx.previewPath, 'thesis.tex');
  assert.ok(cleared >= 3);
}
console.log('reader-races: the preview follows the open file, shows includes inside, and honours the lock');

// Main and secondary previews never expose the retired rendering store.
assert.doesNotMatch(reader, /createRenderingStore|holdRendering|\/renderings\//);

// View dispatches preview, local execution and layout commands at every width.
{
  const commands = [];
  const ctx = context({
    lockPreview: id => commands.push(`lock:${id}`), unlockPreview: () => commands.push('unlock'), openFile: 'paper.tex',
    setLatexOutput: mode => commands.push(`latex-${mode}`),
    chose: value => commands.push(value),
  });
  vm.runInContext(body('  function chooseViewCommand(value)', '  // The File menu.'), ctx);
  vm.runInContext('chooseViewCommand("lock-preview"); chooseViewCommand("unlock-preview"); chooseViewCommand("layout-split")', ctx);
  assert.deepEqual(commands, ['lock:paper.tex','unlock','layout-split']);
  vm.runInContext('chooseViewCommand("preview-latex-html"); chooseViewCommand("preview-latex-pdf")', ctx);
  assert.deepEqual(commands.slice(-2), ['latex-html', 'latex-pdf']);
  const view = body('{#snippet viewItems()}', '{/snippet}');
  const file = body('{#snippet fileItems()}', '{/snippet}');
  assert.match(view, /Lock preview on this file/);
  assert.match(view, /Unlock the preview/);
  assert.match(view, /@render previewItems\(\)/);
  // Running code is the top bar's switch, not a View menu item.
  assert.doesNotMatch(view, /local-execution/);
  assert.doesNotMatch(reader, /preview-quarto|preview-calepin|preview-markdown|preview-typst"/);
  assert.doesNotMatch(file, /preview-/);
  // Settings and Compile now live under File; there is no Tools menu left.
  assert.match(file, /value="settings"/);
  assert.match(file, /value="compile"/);
  assert.doesNotMatch(reader, /label="Tools"/);
  const menus = body('{#snippet menus()}', '{#snippet children()}');
  assert.match(menus, /<MenubarMenu id="file" label="File"/);
  assert.match(menus, /<MenubarMenu id="edit" label="Edit"/);
  assert.match(menus, /<MenubarMenu id="view" label="View"/);
}
console.log('reader-races: View menu exposes explicit preview controls at every width');

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
    history: { loadPage: async () => ({ labels: [], next: null }) },
    fetcher: async (url, options) => {
      posted.push({ url, body: options.body });
      return { ok: true, json: async () => ({}) };
    },
    problem: (message) => assert.fail(message),
  });
  timeline.state.canEdit = true;
  const asShown = JSON.stringify({ main: "main.md", texts: { "main.md": "as confirmed" }, files: {} });
  const moved = JSON.stringify({ main: "main.md", texts: { "main.md": "somebody else typed" }, files: {} });

  // The frontier is the caller's to read and hand over (`Reader.svelte`
  // reads it from the live Loro session); `ask` and `confirm` only carry it.
  timeline.ask("sha-one", asShown, "frontier-as-shown");
  assert.equal(timeline.state.restoring, true);
  await timeline.confirm(moved, "frontier-moved");
  assert.equal(posted.length, 0, "a project that moved is not restored over without a second look");
  assert.equal(timeline.state.restoreMoved, true, "and the dialog says so");
  assert.equal(timeline.state.restoring, true, "and stays open");

  await timeline.confirm(moved, "frontier-moved");
  assert.equal(posted.length, 1, "confirming against what is now on the screen restores");
  const body = JSON.parse(posted[0].body);
  assert.equal(body.sha, "sha-one");
  // §7.1: a restore is refused unless `expected_frontier` equals the head
  // frontier, so the server can tell a concurrent edit nobody here has seen
  // from the moment this browser is actually asking to replace. The dialog
  // reasked once the project moved, so this is the frontier of the project
  // the reader actually confirmed against -- the second reading, not the
  // first.
  assert.equal(body.expected_frontier, "frontier-moved", "the restore names the frontier it was confirmed against");
  assert.equal(timeline.state.restoring, false);
  assert.equal(timeline.state.restoreMoved, false);

  // A reader who cannot edit cannot restore, whatever reaches the action.
  const readOnly = createTimeline({ slug: "doc", fetcher: () => assert.fail("a reader must not restore") });
  readOnly.ask("sha-one", asShown, "frontier-as-shown");
  assert.equal(readOnly.state.restoring, false);
  await readOnly.confirm(asShown, "frontier-as-shown");
}
console.log('reader-races: a restore is confirmed against the project it will replace');
