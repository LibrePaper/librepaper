// Direct behaviour checks for the history controller's async ownership.
import assert from "node:assert/strict";
import { compileModule } from "svelte/compiler";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

// Exercise Svelte's actual state proxies: an identity stub for $state misses
// reactive identity and update bugs in the controller's async guards.
const source = new URL("../src/lib/reader/history.svelte.js", import.meta.url);
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
      if (calls === 1) return [];
      return calls === 2 ? first.promise : second.promise;
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

console.log("history-controller: compiled state, baseline/target/file races, lazy merge ownership and disposal passed");
