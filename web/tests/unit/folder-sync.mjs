// The folder sync scheduler: one run per pause, never two at once.
import assert from "node:assert/strict";
import { createFolderSync } from "../../src/lib/reader/folder-sync.js";

const queue = [];
let nextId = 1;
const timers = {
  set: (fn, ms) => { const id = nextId++; queue.push({ id, fn, ms }); return id; },
  clear: (id) => { const at = queue.findIndex((item) => item.id === id); if (at >= 0) queue.splice(at, 1); },
};
const fire = async () => { const item = queue.shift(); await item.fn(); };

let starts = 0;
let active = 0;
let overlap = false;
let release = null;
const sync = createFolderSync({
  timers,
  run: () => new Promise((resolve) => { starts += 1; active += 1; if (active > 1) overlap = true; release = () => { active -= 1; resolve(); }; }),
});

// Quick changes become one scheduled run, 800 ms out.
sync.poke(); sync.poke(); sync.poke();
assert.equal(queue.length, 1);
assert.equal(queue[0].ms, 800);
const running = fire();
assert.equal(starts, 1);

// A change during the run waits for it, then runs once more.
sync.poke(); sync.poke();
assert.equal(queue.length, 0);
release();
await running;
assert.equal(queue.length, 1);
const second = fire();
assert.equal(starts, 2);
release();
await second;
assert.equal(queue.length, 0);
assert.equal(overlap, false);

// A failed run is reported and the scheduler keeps working.
const errors = [];
const failing = createFolderSync({ timers, run: async () => { throw new Error("boom"); }, onError: (error) => errors.push(error.message) });
failing.poke();
await fire();
assert.deepEqual(errors, ["boom"]);
failing.poke();
assert.equal(queue.length, 1);
failing.dispose();
assert.equal(queue.length, 0);
console.log("folder-sync unit ok");
