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
// flush settles the folder now. The pending run starts without the pause and
// flush waits for it.
const settle = () => new Promise((resolve) => setImmediate(resolve));
{
  let runs = 0;
  let finish = null;
  const quick = createFolderSync({ timers, run: () => new Promise((resolve) => { runs += 1; finish = resolve; }) });
  quick.poke();
  const done = quick.flush();
  assert.equal(queue.length, 0, "flush takes the pending run off the timer");
  assert.equal(runs, 1, "and starts it at once");
  let settled = false;
  done.then(() => { settled = true; });
  await settle();
  assert.equal(settled, false, "flush waits for the run");
  finish();
  assert.equal(await done, true, "and says the folder is in step");

  // With nothing pending and nothing wrong, flush runs nothing.
  assert.equal(await quick.flush(), true);
  assert.equal(runs, 1);

  // A change that arrives during a run is run before flush resolves.
  quick.poke();
  const first = fire();
  assert.equal(runs, 2);
  quick.poke();
  const second = quick.flush();
  finish();
  await settle();
  assert.equal(runs, 3, "the follow-up run starts without waiting for the pause");
  finish();
  assert.equal(await second, true);
  await first;
  assert.equal(queue.length, 0);
  quick.dispose();
}

// A flush that finds the last run failed says so, and a later flush tries once more.
{
  let attempts = 0;
  const flaky = createFolderSync({ timers, run: async () => { attempts += 1; throw new Error("down"); }, onError: () => {} });
  flaky.poke();
  assert.equal(await flaky.flush(), false);
  assert.equal(attempts, 1, "a pending run that fails is not tried twice");
  assert.equal(await flaky.flush(), false);
  assert.equal(attempts, 2, "a flush with nothing pending retries a failed folder");
  flaky.dispose();
}
console.log("folder-sync unit ok");
