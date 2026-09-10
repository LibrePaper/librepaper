// Behavioral checks for the local-preview lifecycle controller, driven under
// Node with fake `local` (the bridge client) functions and a fake timer --
// the same shape of harness `checks/frame-preview.mjs` and
// `checks/reader-races.mjs` already use for Reader's other controllers.
import assert from "node:assert/strict";
import { createLocalPreview } from "../src/lib/reader/local-preview.js";

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
};

function timerHarness() {
  const timers = [];
  const setTimer = (fn, ms) => {
    const timer = { fn, ms, cleared: false };
    timers.push(timer);
    return timer;
  };
  const clearTimer = (timer) => { if (timer) timer.cleared = true; };
  // Runs every timer registered since the last drain, in order, including
  // ones a running callback itself registers -- the same "keep going until
  // nothing new shows up" pump `checks/frame-preview.mjs` uses.
  const flush = async () => {
    let ran = 0;
    const upto = timers.length;
    for (let i = 0; i < upto; i++) {
      const timer = timers[i];
      if (timer.cleared) continue;
      timer.cleared = true;
      timer.fn();
      ran++;
      // Let the callback's promise chain (`.then().catch().finally()`) fully
      // settle before moving on: a macrotask boundary drains every pending
      // microtask, however many hops the chain under test has.
      await new Promise((resolve) => setImmediate(resolve));
    }
    return ran;
  };
  return { timers, setTimer, clearTimer, flush };
}

function notRendered(rendering) {
  const error = new Error("not rendered yet");
  error.name = "NotRendered";
  error.rendering = rendering;
  return error;
}

// start() syncs the workspace, starts the bridge preview, and begins both
// polls; sync() while nothing is running is a no-op.
{
  const { setTimer, clearTimer, flush } = timerHarness();
  const syncs = [];
  const started = [];
  const local = {
    syncWorkspace: async ({ tree }) => { syncs.push(tree); return {}; },
    startLocalPreview: async (request) => { started.push(request); return { id: "p1", url: "http://x/", state: "running" }; },
    stopLocalPreview: async () => {},
    localPreviewStatus: async () => ({ state: "running" }),
    localPreviewPage: async () => ({ rendering: false }),
  };
  const tree = { main: "main.qmd" };
  const running = [];
  const ctl = createLocalPreview({
    local, engine: "quarto",
    publish: () => {}, say: () => {},
    treeNow: () => tree, entrypointOf: (t) => t.main,
    optionsOf: () => ({ format: "html" }), jobOf: () => ({ binding: "hosted" }),
    onRunningChange: (session) => running.push(session),
    setTimer, clearTimer,
  });
  await ctl.sync(); // no-op: nothing running yet
  assert.equal(syncs.length, 0);
  await ctl.start();
  assert.equal(syncs.length, 1);
  assert.equal(started.length, 1);
  assert.equal(started[0].engine, "quarto");
  assert.equal(started[0].options.entrypoint, "main.qmd");
  assert.equal(started[0].options.format, "html");
  assert.equal(started[0].job.binding, "hosted");
  assert.equal(ctl.running, true);
  assert.equal(ctl.id, "p1");
  assert.deepEqual(running.at(-1), { id: "p1", url: "http://x/", state: "running" });
  await flush(); // the page poll's first (0ms) tick
  console.log("local-preview: start() syncs the workspace and starts both polls");
}

// The artifact poll delivers an html page, treats a 304 as "nothing new",
// then delivers a pdf page with the etag's quotes stripped into `sha`.
{
  const { setTimer, clearTimer, flush } = timerHarness();
  const pages = [
    { kind: "html", html: "<p>one</p>", etag: '"e1"', rendering: true },
    { rendering: false }, // a 304
    { kind: "pdf", bytes: new Uint8Array([1, 2, 3]), etag: '"deadbeef"', rendering: false },
  ];
  const local = {
    syncWorkspace: async () => ({}),
    startLocalPreview: async () => ({ id: "p2", state: "running" }),
    stopLocalPreview: async () => {},
    localPreviewStatus: async () => ({ state: "running" }),
    localPreviewPage: async () => pages.shift(),
  };
  const published = [];
  const ctl = createLocalPreview({
    local, engine: "calepin",
    publish: (payload) => published.push(payload), say: () => {},
    treeNow: () => ({ main: "main.typ" }), entrypointOf: (t) => t.main,
    optionsOf: () => ({ format: "pdf" }),
    setTimer, clearTimer,
  });
  await ctl.start();
  await flush(); // html
  await flush(); // 304
  await flush(); // pdf
  assert.equal(published.length, 2, "a 304 publishes nothing");
  assert.deepEqual(published[0], { kind: "html", html: "<p>one</p>" });
  assert.equal(published[1].kind, "pdf");
  assert.equal(published[1].sha, "deadbeef");
  assert.deepEqual([...published[1].bytes], [1, 2, 3]);
  console.log("local-preview: the artifact poll publishes html, skips a 304, and strips the pdf etag into sha");
}

// A NotRendered 404 keeps the poll going rather than ending the session.
{
  const { setTimer, clearTimer, flush } = timerHarness();
  let calls = 0;
  const local = {
    syncWorkspace: async () => ({}),
    startLocalPreview: async () => ({ id: "p3", state: "running" }),
    stopLocalPreview: async () => {},
    localPreviewStatus: async () => ({ state: "running" }),
    localPreviewPage: async () => { calls++; throw notRendered(calls > 2); },
  };
  const published = [];
  const ended = [];
  const ctl = createLocalPreview({
    local, engine: "quarto",
    publish: (payload) => published.push(payload), say: () => {},
    treeNow: () => ({ main: "main.qmd" }), entrypointOf: (t) => t.main,
    optionsOf: () => ({}),
    onEnded: () => ended.push(true),
    setTimer, clearTimer,
  });
  await ctl.start();
  await flush();
  await flush();
  await flush();
  assert.equal(calls, 3, "a 404 re-arms the poll instead of ending the session");
  assert.equal(published.length, 0);
  assert.equal(ended.length, 0);
  assert.equal(ctl.rendering, true, "the third 404 carried a rendering:true header");
  console.log("local-preview: a NotRendered 404 keeps polling and still reflects the rendering flag");
}

// A status other than "running" stops the page poll, deletes the session on
// the bridge, reports the last log line, and calls onEnded.
{
  const { timers, setTimer, clearTimer, flush } = timerHarness();
  const stopped = [];
  const local = {
    syncWorkspace: async () => ({}),
    startLocalPreview: async () => ({ id: "p4", state: "running" }),
    stopLocalPreview: async (id) => stopped.push(id),
    localPreviewStatus: async () => ({ state: "stopped", log_tail: "line one\nline two\n" }),
    localPreviewPage: async () => ({ rendering: false }),
  };
  const said = [];
  const ended = [];
  const ctl = createLocalPreview({
    local, engine: "quarto",
    publish: () => {}, say: (message, problem) => said.push({ message, problem }),
    treeNow: () => ({ main: "main.qmd" }), entrypointOf: (t) => t.main,
    optionsOf: () => ({}),
    onEnded: () => ended.push(true),
    setTimer, clearTimer,
  });
  await ctl.start();
  assert.equal(ctl.running, true);
  // Advance straight to the status poll's own timer, registered separately
  // from the page poll's (which fires at 0ms first).
  const statusTimer = timers.find((t) => t.ms === 5000 && !t.cleared);
  await runTimer(statusTimer);
  assert.equal(ctl.running, false, "a non-running status ends the session");
  assert.deepEqual(stopped, ["p4"]);
  assert.deepEqual(said, [{ message: "line two", problem: true }]);
  assert.deepEqual(ended, [true]);
  console.log("local-preview: a status other than running stops the poll, deletes the session, and calls onEnded");
}

// sync() coalesces concurrent calls into a single trailing retry.
{
  const { setTimer, clearTimer } = timerHarness();
  const gate = [deferred(), deferred(), deferred()];
  let call = 0;
  const syncedTrees = [];
  const local = {
    syncWorkspace: async ({ tree }) => { syncedTrees.push(tree); return gate[call++].promise; },
    startLocalPreview: async () => ({ id: "p5", state: "running" }),
    stopLocalPreview: async () => {},
    localPreviewStatus: async () => ({ state: "running" }),
    localPreviewPage: async () => ({ rendering: false }),
  };
  let tree = { rev: 0 };
  const ctl = createLocalPreview({
    local, engine: "quarto",
    publish: () => {}, say: () => {},
    treeNow: () => tree, entrypointOf: () => "main.qmd",
    optionsOf: () => ({}),
    setTimer, clearTimer,
  });
  const starting = ctl.start(); // consumes gate[0], syncedTrees[0] = {rev:0}
  gate[0].resolve({});
  await starting;
  tree = { rev: 1 };
  const first = ctl.sync(); // busy: starts a new syncWorkspace immediately
  tree = { rev: 2 };
  const second = ctl.sync(); // arrives mid-flight: queued
  const third = ctl.sync(); // arrives mid-flight too: coalesced with the queued one
  gate[1].resolve({});
  await first;
  await second;
  await third;
  gate[2].resolve({});
  await new Promise(setImmediate);
  assert.equal(syncedTrees.length, 3, "one call ran immediately, the rest coalesced into a single trailing retry");
  assert.deepEqual(syncedTrees[1], { rev: 1 });
  assert.deepEqual(syncedTrees[2], { rev: 2 }, "the trailing retry syncs the latest tree, not the one queued first");
  console.log("local-preview: sync() serializes concurrent calls into one trailing retry");
}

function runTimer(timer) {
  timer.cleared = true;
  timer.fn();
  return new Promise((resolve) => setImmediate(resolve));
}

console.log("local-preview: all checks passed");
