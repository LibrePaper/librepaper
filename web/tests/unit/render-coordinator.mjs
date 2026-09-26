import assert from "node:assert/strict";
import { createRenderCoordinator } from "../../src/lib/reader/render-coordinator.js";

let navigation = 1;
let source = 1;
let main = "main.md";
const started = [];
const releases = [];
const coordinator = createRenderCoordinator({
  navigation: () => navigation,
  source: () => source,
  main: () => main,
  render: async (ticket) => {
    started.push(ticket);
    await new Promise((resolve) => releases.push(resolve));
  },
});

const first = coordinator.request();
assert.deepEqual(started, [1]);
assert.equal(coordinator.running, true);
const second = coordinator.request();
const third = coordinator.request();
assert.equal(coordinator.queued, true);
releases.shift()();
await new Promise(setImmediate);
assert.deepEqual(started, [1, 3], "a burst coalesces into the newest request");
releases.shift()();
await Promise.all([first, second, third]);
await new Promise(setImmediate);
assert.equal(coordinator.running, false);

assert.equal(coordinator.commit(1), true);
assert.equal(coordinator.commit(1), false);
assert.equal(coordinator.isCommitted(1), true);
assert.equal(coordinator.isNewer(2), true);
assert.equal(coordinator.superseded({
  ticket: 2, capturedNavigation: 1, capturedSource: 1,
  strictSource: false, capturedMain: "main.md",
}), false);
source = 2;
assert.equal(coordinator.superseded({
  ticket: 2, capturedNavigation: 1, capturedSource: 1,
  strictSource: false, capturedMain: "main.md",
}), false, "fast renders may show intermediate source while the queued render catches up");
assert.equal(coordinator.superseded({
  ticket: 2, capturedNavigation: 1, capturedSource: 1,
  strictSource: true, capturedMain: "main.md",
}), true);
navigation = 2;
assert.equal(coordinator.superseded({
  ticket: 2, capturedNavigation: 1, capturedSource: 2,
  strictSource: false, capturedMain: "main.md",
}), true);
navigation = 1;
main = "chapter.md";
assert.equal(coordinator.superseded({
  ticket: 2, capturedNavigation: 1, capturedSource: 2,
  strictSource: false, capturedMain: "main.md",
}), true);

console.log("render coordinator tests passed");
