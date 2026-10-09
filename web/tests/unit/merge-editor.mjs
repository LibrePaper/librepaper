// importOnce shares one load between concurrent callers, and forgets a
// failed load so the next call can try again.

import assert from "node:assert/strict";
import { importOnce } from "../../src/lib/merge-editor.js";

// --- concurrent calls share one load -----------------------------------------

let runs = 0;
let resolveFirst;
const once = importOnce(() => {
  runs += 1;
  return new Promise((resolve) => (resolveFirst = resolve));
});

const first = once();
const second = once();
assert.equal(first, second, "a call before the first resolves gets the same promise");
assert.equal(runs, 1, "load ran once");
resolveFirst("component");
assert.equal(await first, "component");

// --- a rejection is forgotten -------------------------------------------------

let attempts = 0;
const retry = importOnce(async () => {
  attempts += 1;
  if (attempts === 1) throw new Error("chunk missing");
  return "component";
});

await assert.rejects(retry(), /chunk missing/);
assert.equal(await retry(), "component", "the next call runs load again and succeeds");
assert.equal(attempts, 2, "load ran a second time after the rejection");

console.log("merge-editor: shared load and retry after failure passed");
