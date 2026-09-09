// Direct behaviour checks for the history controller's async ownership.
import assert from "node:assert/strict";

// Svelte supplies this rune when the module is compiled. Supplying the same
// identity initializer lets this direct behaviour check import the module in
// Node while still exercising the production controller source.
globalThis.$state = (initial) => initial;
const { createHistoryController } = await import("../src/lib/reader/history.svelte.js");

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

console.log("history-controller: stale baseline/target/file diff and disposal guards passed");
