// Walking back through versions to find where an orphaned passage went.
//
// The walk makes one request per lost comment, so it can still be running
// when the source changes under it. What is checked here is that a stale walk
// writes nothing, and that a walk which would reach the same answers is not
// made twice.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";
import * as realPassages from "../../src/lib/passages.js";

const { createPassageTrace } = await loadRunes(
  new URL("../../src/lib/reader/passage-trace.svelte.js", import.meta.url),
);

// A lost comment as the server records it: what it is about, in the file it
// was about, at the checkpoint it was made on.
const orphan = (id, checkpoint = "r1") => ({
  id,
  orphaned: true,
  original_anchor: {
    kind: "source_text",
    checkpoint_id: checkpoint,
    target: { file_id: id, exact: id, prefix: "", suffix: "" },
  },
});
const tree = { texts: { "main.md": "current source" } };

function build({ current, passages = {}, loadCheckpoints = async () => [] } = {}) {
  const calls = [];
  const trace = createPassageTrace({
    slug: "doc",
    now: () => current(),
    loadCheckpoints,
    passages: {
      tracedBy: realPassages.tracedBy,
      wentAt: async (_slug, traced) => { calls.push(`went:${traced.selector.exact}`); return { sha: "sha-1" }; },
      sourceTextAt: async () => "old source",
      replacementAt: async () => "what stands there now",
      ...passages,
    },
  });
  return { trace, calls };
}

// The ordinary walk: each lost comment gets a version and a replacement.
{
  let state = { source: 1, visible: "visible" };
  const { trace, calls } = build({ current: () => state });
  await trace.trace({ comments: [orphan("a"), orphan("b")], tree, checkpoints: [{ sha: "sha-1" }] });
  assert.deepEqual(calls, ["went:a", "went:b"]);
  assert.deepEqual(Object.keys(trace.state.went), ["a", "b"]);
  assert.deepEqual(trace.state.replacements, { a: "what stands there now", b: "what stands there now" });
}

// A comment that still anchors is not walked, and neither is one about the
// document as a whole: it has no passage that could have gone anywhere.
{
  let state = { source: 1, visible: "visible" };
  const { trace, calls } = build({ current: () => state });
  await trace.trace({
    comments: [
      { id: "fine", orphaned: false },
      { id: "whole", orphaned: true, original_anchor: { kind: "document", checkpoint_id: "r1" } },
    ],
    tree,
    checkpoints: [{ sha: "sha-1" }],
  });
  assert.deepEqual(calls, []);
}

// The same walk is not made twice: the source, the visible text and the
// comments together say whether the answers could differ.
{
  let state = { source: 1, visible: "visible" };
  const { trace, calls } = build({ current: () => state });
  const comments = [orphan("a")];
  await trace.trace({ comments, tree, checkpoints: [{ sha: "sha-1" }] });
  await trace.trace({ comments, tree, checkpoints: [{ sha: "sha-1" }] });
  assert.deepEqual(calls, ["went:a"], "an identical walk is not repeated");
  state = { source: 2, visible: "visible" };
  await trace.trace({ comments, tree, checkpoints: [{ sha: "sha-1" }] });
  assert.deepEqual(calls, ["went:a", "went:a"], "a changed source is walked again");
}

// A source that changes mid-walk abandons it: what it found describes text
// nobody is looking at any more.
{
  let state = { source: 1, visible: "visible" };
  const { trace } = build({
    current: () => state,
    passages: {
      wentAt: async () => { state = { source: 2, visible: "visible" }; return { sha: "sha-1" }; },
    },
  });
  await trace.trace({ comments: [orphan("a")], tree, checkpoints: [{ sha: "sha-1" }] });
  assert.deepEqual(trace.state.went, {}, "a walk overtaken by an edit writes nothing");
}

// So does a walk overtaken by a newer one, even when the source is unchanged.
{
  let state = { source: 1, visible: "visible" };
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  let first = true;
  const { trace } = build({
    current: () => state,
    passages: {
      wentAt: async (_slug, comment) => {
        if (first) { first = false; await held; }
        return { sha: `sha-${comment.id}` };
      },
    },
  });
  const slow = trace.trace({ comments: [orphan("a")], tree, checkpoints: [{ sha: "sha-1" }] });
  await trace.trace({ comments: [orphan("b")], tree, checkpoints: [{ sha: "sha-1" }] });
  release();
  await slow;
  assert.deepEqual(Object.keys(trace.state.went), ["b"], "the newer walk's answers stand");
}

// The manifest is read only when a walk needs one and none has been read.
{
  let state = { source: 1, visible: "visible" };
  let loads = 0;
  const { trace } = build({
    current: () => state,
    loadCheckpoints: async () => { loads++; return [{ sha: "sha-1" }]; },
  });
  await trace.trace({ comments: [], tree, checkpoints: [] });
  assert.equal(loads, 0, "nothing lost, nothing to read the manifest for");
  await trace.trace({ comments: [orphan("a")], tree, checkpoints: [] });
  assert.equal(loads, 1);
  state = { source: 2, visible: "visible" };
  await trace.trace({ comments: [orphan("a")], tree, checkpoints: [{ sha: "sha-1" }] });
  assert.equal(loads, 1, "a manifest already in hand is used");
}

// A version that cannot be read establishes nothing, and the walk is
// forgotten rather than recorded, so the next one tries again.
{
  let state = { source: 1, visible: "visible" };
  let attempts = 0;
  const { trace } = build({
    current: () => state,
    passages: {
      wentAt: async () => { attempts++; throw new Error("that checkpoint is gone"); },
    },
  });
  const comments = [orphan("a")];
  await trace.trace({ comments, tree, checkpoints: [{ sha: "sha-1" }] });
  await trace.trace({ comments, tree, checkpoints: [{ sha: "sha-1" }] });
  assert.equal(attempts, 2, "a failed walk is tried again rather than remembered");
  assert.deepEqual(trace.state.went, {});
}

console.log("passage trace: where a lost passage went, and which walk may say so");
