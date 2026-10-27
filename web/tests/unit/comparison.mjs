// The comparison the editor area shows in place of its source, checked without
// a browser. The rule under test is that starting one saves the editor area as
// it was, and that ending one hands that view back, once.

import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createComparison } = await loadRunes(new URL("../../src/lib/reader/comparison.svelte.js", import.meta.url));

{
  // Nothing is open at the start, and ending nothing gives nothing back.
  const comparison = createComparison();
  assert.equal(comparison.kind, "");
  assert.equal(comparison.end(), null);
}

{
  // Beginning reads the view once, through the reader's own function, and
  // ending hands that view back, and only once.
  const comparison = createComparison();
  const view = { editing: true, layout: "split", openFile: "main.typ", caret: 12 };
  let reads = 0;
  comparison.begin("history", () => { reads += 1; return view; });
  assert.equal(comparison.kind, "history");
  assert.equal(reads, 1);
  assert.equal(comparison.end(), view);
  assert.equal(comparison.kind, "");
  assert.equal(comparison.end(), null, "a saved view is handed back once");
  assert.equal(reads, 1, "ending does not read the view again");
}

{
  // The view is what the reader had when the comparison began, not what it has
  // become since.
  const comparison = createComparison();
  let openFile = "main.typ";
  comparison.begin("history", () => ({ openFile }));
  openFile = "chapter.typ";
  assert.equal(comparison.end().openFile, "main.typ");
}

{
  // Switching from one version to another while a comparison is open keeps the
  // view from before the comparison. The second begin must not save the
  // comparison's own view as the one to return to.
  const comparison = createComparison();
  const before = { editing: true, layout: "split", openFile: "main.typ" };
  comparison.begin("history", () => before);
  let reads = 0;
  comparison.begin("history", () => { reads += 1; return { editing: true, layout: "source", openFile: "other.typ" }; });
  assert.equal(reads, 0, "a second begin does not read the view");
  assert.equal(comparison.kind, "history");
  assert.equal(comparison.end(), before);
}

console.log("comparison: ok");
