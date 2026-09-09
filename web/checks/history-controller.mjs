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

// Chained attribution: two checkpoints in the range, each by a different
// author, editing different offsets -- the range-level hunks end up with a
// `who` each, matching the author of the step that made them, not just the
// single range-level name `attribution()` would give both.
{
  // vincent inserts "AAA" at base-text offset 10; sam separately inserts
  // "BBB" at offset 20 of the text vincent's edit produced ("one"), which is
  // offset 17 of the original base text once vincent's 3 inserted
  // characters before it are accounted for -- exactly what the combined
  // base-to-"two" diff below says.
  const texts = { base: "TEXT_BASE", one: "TEXT_ONE", two: "TEXT_TWO" };
  const wordDiffOf = {
    "base|one": [{ at: 10, delete: 0, insert: "AAA" }],
    "one|two": [{ at: 20, delete: 0, insert: "BBB" }],
    "base|two": [{ at: 10, delete: 0, insert: "AAA" }, { at: 17, delete: 0, insert: "BBB" }],
  };
  const api = {
    load: async () => [
      { sha: "base", by: "nobody" },
      { sha: "one", by: "vincent" },
      { sha: "two", by: "sam" },
    ],
    checkpoint: async (_slug, sha) => point(sha, texts[sha]),
    wordDiff: async (oldText, newText) => {
      const key = `${Object.keys(texts).find((k) => texts[k] === oldText)}|${Object.keys(texts).find((k) => texts[k] === newText)}`;
      return wordDiffOf[key] || [];
    },
    hunks,
  };
  const session = liveSession(texts.two);
  const controller = createHistoryController({
    slug: "doc",
    history: api,
    passages: { textAt: async (_slug, sha) => texts[sha] },
    live: () => ({ session, text: texts.two }),
    readBaseline: () => "base",
    rememberBaseline: () => {},
  });
  await controller.load();
  await controller.chooseTarget("two");
  assert.equal(controller.changes.length, 2);
  assert.equal(controller.changes[0].who, "vincent");
  assert.equal(controller.changes[1].who, "sam");
}

console.log("history-controller: compiled state, baseline/target/file races, compareTo, lazy merge ownership and disposal passed");
