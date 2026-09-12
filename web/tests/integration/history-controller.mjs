// Direct behaviour checks for the history controller's async ownership.
import assert from "node:assert/strict";
import { compileModule } from "svelte/compiler";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { projectText } from "../../src/lib/diff-display.js";

// Exercise Svelte's actual state proxies: an identity stub for $state misses
// reactive identity and update bugs in the controller's async guards.
const source = new URL("../../src/lib/reader/history.svelte.js", import.meta.url);
const directory = mkdtempSync(join(tmpdir(), "librepaper-history-controller-"));
let createHistoryController;
try {
  const compiled = compileModule(readFileSync(source, "utf8"), { filename: source.pathname, generate: "client" });
  const code = compiled.js.code.replace(/from (["'])([^"']+)\1/g, (_match, _quote, specifier) =>
    `from ${JSON.stringify(specifier.startsWith(".") ? new URL(specifier, source).href : import.meta.resolve(specifier))}`);
  const output = join(directory, "history.mjs");
  writeFileSync(output, code);
  ({ createHistoryController } = await import(pathToFileURL(output).href));
} finally {
  rmSync(directory, { recursive: true, force: true });
}

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => (resolve = done));
  return { promise, resolve };
};

const point = (sha, text = sha) => ({
  sha,
  main: "main.md",
  texts: { "main.md": text },
  files: { "main.md": { kind: "text" } },
});

const liveSession = (text = "live") => ({
  tree: () => point("live", text),
  idOf: () => "main",
  textOf: () => text,
  awareness: {},
});

const hunks = (_old, _new, edits) => edits.map((edit) => ({
  at: edit.at,
  delete: edit.delete,
  insert: edit.insert,
  old: "",
  before: "",
  after: "",
  current: edit.insert,
  currentBefore: "",
  currentAfter: "",
}));

// The metadata-bearing adapter reaches the controller without changing the
// list-only shape accepted by older history integrations.
{
  const controller = createHistoryController({
    slug: "doc",
    history: {
      loadWithStatus: async () => ({
        checkpoints: [point("A", "saved")],
        durability: { live_save: "saved", history_checkpoint: "pending" },
      }),
      checkpoint: async (_slug, sha) => point(sha, "saved"),
      hunks,
    },
    passages: { textAt: async () => "saved" },
    live: () => ({ session: liveSession("saved"), text: "saved" }),
  });
  await controller.load();
  assert.deepEqual(controller.durability, {
    live_save: "saved",
    history_checkpoint: "pending",
  });
}

// A late baseline response cannot overwrite a newer selection.
{
  const a = deferred();
  const b = deferred();
  const api = {
    load: async () => [{ sha: "A" }, { sha: "B" }],
    checkpoint: (_slug, sha) => sha === "A" ? a.promise : b.promise,
    wordDiff: async () => [],
    hunks,
  };
  const controller = createHistoryController({
    slug: "doc",
    history: api,
    passages: { textAt: async () => "same" },
    live: () => ({ session: liveSession(), text: "same" }),
  });
  const loading = controller.load();
  await Promise.resolve();
  const newer = controller.chooseBaseline("B");
  b.resolve(point("B"));
  await newer;
  a.resolve(point("A"));
  await loading;
  assert.equal(controller.baseline.sha, "B");
}

// A late target response cannot replace the newer comparison target.
{
  const b = deferred();
  const c = deferred();
  const api = {
    load: async () => [point("A"), { sha: "B" }, { sha: "C" }],
    checkpoint: (_slug, sha) => sha === "B" ? b.promise : c.promise,
    wordDiff: async () => [],
    hunks,
  };
  const controller = createHistoryController({
    slug: "doc",
    history: api,
    passages: { textAt: async () => "same" },
    live: () => ({ session: liveSession(), text: "same" }),
  });
  await controller.load();
  const oldTarget = controller.chooseTarget("B");
  const newTarget = controller.chooseTarget("C");
  c.resolve(point("C"));
  await newTarget;
  b.resolve(point("B"));
  await oldTarget;
  assert.equal(controller.target.sha, "C");
}

// A late file diff cannot overwrite a newer request for the same file.
{
  const first = deferred();
  const second = deferred();
  let calls = 0;
  const session = liveSession("new");
  const api = {
    load: async () => [point("A", "old")],
    wordDiff: async () => {
      calls += 1;
      return calls === 1 ? first.promise : second.promise;
    },
    hunks,
  };
  const controller = createHistoryController({
    slug: "doc",
    history: api,
    passages: { textAt: async () => "old" },
    live: () => ({ session, text: "new" }),
  });
  await controller.load();
  const oldDiff = controller.openFileDiff("main.md");
  await Promise.resolve();
  const newDiff = controller.openFileDiff("main.md");
  second.resolve([{ at: 0, delete: 3, insert: "new" }]);
  await newDiff;
  first.resolve([{ at: 0, delete: 3, insert: "old" }]);
  await oldDiff;
  assert.equal(controller.fileDiff.new, "new");
  assert.equal(controller.fileDiff.hunks[0].insert, "new");
}

// Disposal invalidates an in-flight checkpoint and leaves controller state
// untouched when the request eventually resolves.
{
  const pending = deferred();
  const controller = createHistoryController({
    slug: "doc",
    history: {
      load: async () => [{ sha: "A" }],
      checkpoint: async () => pending.promise,
      wordDiff: async () => [],
      hunks,
    },
    live: () => ({ session: liveSession(), text: "same" }),
  });
  const loading = controller.load();
  await Promise.resolve();
  controller.dispose();
  pending.resolve(point("A"));
  await loading;
  assert.equal(controller.baseline, null);
  assert.deepEqual(controller.checkpoints, [{ sha: "A" }]);
}

// Loading a new manifest invalidates its predecessor's automatic baseline,
// including when the new manifest is empty and starts no baseline request.
{
  const old = deferred();
  let loads = 0;
  const controller = createHistoryController({
    slug: "doc",
    history: {
      load: async () => ++loads === 1 ? [{ sha: "old" }] : [],
      checkpoint: () => old.promise,
    },
  });
  const first = controller.load();
  await Promise.resolve();
  await controller.load();
  old.resolve(point("old"));
  await first;
  assert.equal(controller.baseline, null);
  assert.deepEqual(controller.checkpoints, []);
}

// The lazy merge component must open for a current editable comparison, but
// may not reopen after closing, revocation, disposal or a baseline switch.
for (const outcome of ["current", "closed", "revoked", "disposed", "baseline"]) {
  const component = deferred();
  const started = deferred();
  const session = liveSession("new");
  let mayEdit = true;
  let shown = null;
  const controller = createHistoryController({
    slug: "doc",
    live: () => ({ session, text: "new" }),
    mayEdit: () => mayEdit,
    editing: () => true,
    history: { load: async () => [point("A", "old"), point("B", "other")], wordDiff: async () => [], hunks, shortSha: (sha) => sha },
    passages: { textAt: async () => "old" },
    onMerge: async (target, current) => {
      if (!target) { shown = null; return; }
      started.resolve();
      await component.promise;
      if (current()) shown = target;
    },
  });
  await controller.load();
  const opening = controller.openFileDiff("main.md");
  await started.promise;
  if (outcome === "closed") controller.closeFileDiff();
  if (outcome === "revoked") mayEdit = false;
  if (outcome === "disposed") controller.dispose();
  if (outcome === "baseline") await controller.chooseBaseline("B");
  component.resolve();
  await opening;
  assert.equal(Boolean(shown), outcome === "current", outcome);
  if (shown) {
    assert.equal(shown.oldText, "old");
    assert.equal(shown.newText, "new");
    assert.equal(shown.editable, true);
    assert.equal(controller.fileDiff, null);
  }
}

// A target fetched from an older manifest cannot commit after refreshing it.
{
  const target = deferred();
  let loads = 0;
  const controller = createHistoryController({
    slug: "doc",
    history: {
      load: async () => ++loads === 1 ? [point("A"), { sha: "B" }] : [point("A")],
      checkpoint: () => target.promise,
    },
  });
  await controller.load();
  const selecting = controller.chooseTarget("B");
  await controller.load();
  target.resolve(point("B"));
  await selecting;
  assert.equal(controller.target, null);
}

// The timeline's click: the row becomes the compare end, and the baseline
// steps back before it when it would otherwise not be before it.
{
  const controller = createHistoryController({
    slug: "doc",
    history: {
      load: async () => [{ sha: "A" }, { sha: "B" }, { sha: "C" }],
      checkpoint: async (_slug, sha) => point(sha),
      wordDiff: async () => [],
      hunks,
    },
    passages: { textAt: async () => "same" },
    live: () => ({ session: liveSession(), text: "same" }),
    readBaseline: () => "B",
    rememberBaseline: () => {},
  });
  await controller.load();
  assert.equal(controller.baseline.sha, "B");
  await controller.compareTo("C");
  assert.equal(controller.baseline.sha, "B");
  assert.equal(controller.target.sha, "C");
  // An older row than the baseline: what that checkpoint itself changed.
  await controller.compareTo("A");
  assert.equal(controller.baseline.sha, "A", "the first checkpoint compares with itself");
  assert.equal(controller.target.sha, "A");
  await controller.compareTo("B");
  assert.equal(controller.baseline.sha, "A");
  assert.equal(controller.target.sha, "B");
  // Moving the baseline past the compare end runs the range to the live document.
  await controller.chooseBaseline("C");
  assert.equal(controller.target, null);
  await controller.compareTo("");
  assert.equal(controller.target, null);
  assert.equal(controller.redlines, true, "the changes are painted from the start");
}

// Rendered comparisons ask for exactly the selected endpoints. The target is
// requested first so a clean selected preview is not held behind the baseline
// on a single-slot HTML renderer.
{
  const target = deferred();
  const baseline = deferred();
  const calls = [];
  const api = {
    load: async () => [point("base", "base"), point("target", "target")],
    checkpoint: async (_slug, sha) => point(sha, sha),
    hunks,
  };
  const session = liveSession("target");
  const controller = createHistoryController({
    slug: "doc",
    history: api,
    passages: {
      projectionAt: async (_slug, sha) => {
        calls.push(sha);
        return sha === "target" ? target.promise : baseline.promise;
      },
    },
    live: () => ({ session, text: "target" }),
    readBaseline: () => "base",
    rememberBaseline: () => {},
  });
  await controller.load();
  const selecting = controller.chooseTarget("target");
  await Promise.resolve();
  assert.deepEqual(calls, ["target"]);
  target.resolve(projectText("target"));
  for (let tick = 0; tick < 10 && calls.length < 2; tick++) await Promise.resolve();
  assert.deepEqual(calls, ["target", "base"]);
  baseline.resolve(projectText("base"));
  await selecting;
  assert.equal(controller.changes.length, 1);
}

// An identical background refresh must not replace a pending user compare
// with the panel's non-computing default baseline selection.
{
  const initial = deferred(), explicit = deferred();
  let reads = 0;
  const session = liveSession("current");
  const controller = createHistoryController({
    slug: "doc", readBaseline: () => "base", rememberBaseline: () => {},
    history: {
      load: async () => [{ sha: "base" }],
      checkpoint: () => (++reads === 1 ? initial.promise : explicit.promise), hunks,
    },
    passages: { projectionTree: async () => projectText("current"), projectionAt: async () => projectText("old") },
    live: () => ({ session, text: "current", tree: point("live", "current") }),
  });
  const loading = controller.load();
  await Promise.resolve();
  const comparing = controller.chooseBaseline("base");
  await controller.load();
  assert.equal(reads, 2, "refresh does not start a third default selection");
  initial.resolve(point("base", "old"));
  explicit.resolve(point("base", "old"));
  await Promise.all([loading, comparing]);
  assert.equal(controller.capturedCurrent.texts["main.md"], "current");
  assert.ok(controller.changes.length > 0);
  controller.dispose();
}

// A captured-current target notifies Reader before the baseline projection;
// the clean target can therefore be painted while the comparison continues.
{
  const calls = [];
  const session = liveSession("current");
  const controller = createHistoryController({
    slug: "doc",
    history: {
      load: async () => [point("base", "base")],
      checkpoint: async () => point("base", "base"),
      hunks,
    },
    passages: {
      projectionTree: async () => { calls.push("target"); return projectText("current"); },
      projectionAt: async () => { calls.push("baseline"); return projectText("base"); },
    },
    live: () => ({ session, text: "current", tree: point("live", "current") }),
    readBaseline: () => "base",
    onTargetReady: () => calls.push("ready"),
  });
  await controller.load();
  await controller.compareWithCurrent("base");
  assert.ok(calls.indexOf("ready") >= 0);
  assert.ok(calls.indexOf("ready") < calls.indexOf("baseline"));
}

// Historical HTML can finish before the initial live Yjs synchronization.
// Comparison must capture the joined state, never the temporary empty tree.
{
  let text = "";
  const session = { joined: false, tree: () => point("live", text) };
  const controller = createHistoryController({
    slug: "doc",
    history: { load: async () => [point("base", "old")], checkpoint: async () => point("base", "old"), hunks },
    passages: { projectionTree: async (tree) => projectText(tree.texts[tree.main]), projectionAt: async () => projectText("old") },
    live: () => ({ session, text, tree: session.tree() }),
    readBaseline: () => "base",
    rememberBaseline: () => {},
  });
  await controller.load();
  const comparing = controller.compareWithCurrent("base");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(controller.capturedCurrent, null);
  text = "synchronized current";
  session.joined = true;
  await comparing;
  assert.equal(controller.capturedCurrent.texts["main.md"], text);
  controller.dispose();
}

// A slower endpoint from an older selection cannot commit after a newer
// selection advances the comparison generation.
{
  const oldTarget = deferred();
  const newTarget = deferred();
  const api = {
    load: async () => [point("A", "base"), point("B", "old"), point("C", "new")],
    checkpoint: async (_slug, sha) => point(sha),
    hunks,
  };
  const controller = createHistoryController({
    slug: "doc",
    history: api,
    passages: {
      projectionAt: async (_slug, sha) => sha === "B" ? oldTarget.promise
        : sha === "C" ? newTarget.promise : projectText("base"),
    },
    live: () => ({ session: liveSession("new"), text: "new" }),
  });
  await controller.load();
  const oldSelection = controller.chooseTarget("B");
  await Promise.resolve();
  const newSelection = controller.chooseTarget("C");
  await Promise.resolve();
  newTarget.resolve(projectText("new"));
  await newSelection;
  oldTarget.resolve(projectText("old"));
  await oldSelection;
  assert.equal(controller.target.sha, "C");
}

// Explicit current comparisons capture both the target tree and its text.
// Incoming edits are visible as a prompt, not silently incorporated; refresh
// deliberately creates a new target. Ordinary selection then returns to the
// predecessor rule.
{
  let currentText = "live one";
  const asset = Uint8Array.of(1, 2, 3);
  const tree = {
    ...point("live", currentText),
    settings: { release: "r1", engine: "auto" },
    digests: { "figure.png": "digest-r1" },
    assets: { "figure.png": asset },
  };
  const session = { tree: () => tree, idOf: () => "main", textOf: () => currentText, awareness: {} };
  const controller = createHistoryController({
    slug: "doc",
    history: {
      load: async () => [point("A", "old"), point("B", "middle"), point("C", "new")],
      wordDiff: async () => [],
      hunks,
    },
    passages: {
      textAt: async (_slug, sha) => ({ A: "old", B: "middle", C: "new" }[sha]),
      captureTree: async (_slug, value) => ({
        ...value,
        settings: { ...value.settings },
        digests: { ...value.digests },
        assets: Object.fromEntries(Object.entries(value.assets || {}).map(([path, bytes]) => [path, bytes.slice()])),
        urls: { "figure.png": "blob:figure" },
      }),
    },
    live: () => ({ session, text: currentText }),
    snapshotDigest: async () => "captured-current",
    readBaseline: () => "A",
    rememberBaseline: () => {},
  });
  await controller.load();
  await controller.compareWithCurrent("A");
  const captured = controller.capturedCurrent;
  assert.equal(controller.comparingCurrent, true);
  assert.equal(captured.texts["main.md"], "live one");
  assert.equal(captured.settings.release, "r1");
  assert.notEqual(captured.assets["figure.png"], asset);
  assert.equal(captured.assets["figure.png"][0], 1);
  assert.deepEqual(controller.changedPaths, ["main.md"], "file/source paths remain available when rendered text is unavailable");
  assert.equal(controller.newerEdits, false);
  currentText = "live two";
  tree.texts["main.md"] = currentText;
  tree.settings.release = "r2";
  tree.digests["figure.png"] = "digest-r2";
  asset[0] = 9;
  controller.noteLiveChange();
  assert.equal(controller.newerEdits, true);
  await controller.refreshCurrent();
  assert.equal(controller.newerEdits, false);
  assert.equal(controller.capturedCurrent.texts["main.md"], "live two");
  assert.equal(controller.capturedCurrent.settings.release, "r2");
  assert.equal(controller.capturedCurrent.assets["figure.png"][0], 9);
  await controller.compareTo("C");
  assert.equal(controller.comparingCurrent, false);
  assert.equal(controller.baseline.sha, "B");
  assert.equal(controller.target.sha, "C");
}

// A delayed current-tree digest cannot commit after a newer ordinary click.
{
  const digest = deferred();
  const controller = createHistoryController({
    slug: "doc",
    history: { load: async () => [point("A"), point("B")], wordDiff: async () => [], hunks },
    passages: { textAt: async () => "same" },
    live: () => ({ session: liveSession("live"), text: "live" }),
    snapshotDigest: async () => digest.promise,
    readBaseline: () => "A",
    rememberBaseline: () => {},
  });
  await controller.load();
  const pending = controller.compareWithCurrent("A");
  await Promise.resolve();
  await controller.compareTo("B");
  digest.resolve("current-digest");
  await pending;
  assert.equal(controller.comparingCurrent, false);
  assert.equal(controller.target.sha, "B");
}

console.log("history-controller: compiled state, baseline/target/file races, compareTo, lazy merge ownership and disposal passed");
