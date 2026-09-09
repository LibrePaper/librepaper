import assert from "node:assert/strict";
import { createFramePreview } from "../src/lib/reader/frame-preview.js";
import { createRenderingStore } from "../src/lib/reader/rendering-store.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => (resolve = done));
  return { promise, resolve };
};

function timerHarness() {
  const timers = [];
  const setTimer = (fn, ms) => {
    const timer = { fn, ms, cleared: false };
    timers.push(timer);
    return timer;
  };
  const clearTimer = (timer) => {
    if (timer) timer.cleared = true;
  };
  return { timers, setTimer, clearTimer };
}

// A token response for an old navigation must not install its URL over the
// newer navigation, and a payload fetched before readiness must replay with a
// fresh transferable buffer.
{
  const passResponses = [deferred(), deferred()];
  const passes = [...passResponses];
  const urls = [];
  const sent = [];
  let kind = "raw";
  const frame = createFramePreview({
    slug: "doc",
    getDocsOrigin: () => "https://docs.test",
    framePath: () => kind,
    api: { frame: () => passResponses.shift().promise },
    setSource: (source) => urls.push(source),
    send: (message) => sent.push(message),
  });
  frame.navigate();
  frame.navigate(true);
  passes[0].resolve({ ok: true, json: async () => ({ token: "old", until: 1 }) });
  passes[1].resolve({ ok: true, json: async () => ({ token: "new", until: 2 }) });
  await new Promise((resolve) => setTimeout(resolve, 0));
  kind = "pdf";
  assert.equal(urls.at(-1), "https://docs.test/raw/doc/?v=2&until=2&token=new");
  frame.navigate(true);

  // The current PDF navigation is ready only after markReady.
  frame.markReady();
  const original = Uint8Array.of(4, 5);
  frame.publish({ kind: "pdf", sha: "a", bytes: original });
  assert.equal(sent.length, 1);
  new Uint8Array(sent[0].pdf)[0] = 99;
  assert.equal(frame.preview().bytes[0], 4);
  frame.dispose();
  assert.equal(frame.publish({ kind: "pdf", sha: "b", bytes: Uint8Array.of(1) }), false);
  assert.equal(frame.replay(), false);
  assert.equal(frame.markReady(), false);
  assert.equal(frame.navigate(true), false);
}

// A late rendering fetch cannot overwrite a newer request, and disposal
// prevents a quiet timer from publishing bytes after the component leaves.
{
  const latestResponses = [deferred(), deferred()];
  const latest = [...latestResponses];
  const renderingBytes = { old: deferred(), new: deferred() };
  const states = [];
  const delivered = [];
  const timer = timerHarness();
  let viewing = null;
  let source = 0;
  let navigation = 0;
  const store = createRenderingStore({
    api: {
      latest: () => latestResponses.shift().promise,
      rendering: (sha) => renderingBytes[sha].promise,
      putRendering: async () => ({ ok: true }),
    },
    getViewing: () => viewing,
    getSourceGeneration: () => source,
    getNavigationGeneration: () => navigation,
    deliver: (value) => delivered.push(value),
    onRendering: (value) => states.push(value),
    onMissing: () => {},
    schedulePreview: () => {},
    setTimer: timer.setTimer,
    clearTimer: timer.clearTimer,
  });
  const first = store.paint();
  await Promise.resolve();
  const second = store.paint();
  await Promise.resolve();
  latest[0].resolve({ ok: true, json: async () => ({ sha: "old" }) });
  latest[1].resolve({ ok: true, json: async () => ({ sha: "new" }) });
  await Promise.resolve();
  await Promise.resolve();
  renderingBytes.old.resolve({ ok: true, arrayBuffer: async () => Uint8Array.of(1).buffer });
  renderingBytes.new.resolve({ ok: true, arrayBuffer: async () => Uint8Array.of(2).buffer });
  await Promise.all([first, second]);
  assert.equal(delivered.length, 1);
  assert.equal(delivered[0].sha, "new");
  assert.equal(delivered[0].bytes[0], 2);

  const held = store.hold("quiet", Uint8Array.of(8), null, false);
  source += 1;
  store.dispose();
  await held;
  assert.equal(states.some((state) => state?.sha === "quiet"), false);
  const latestCount = latestResponses.length;
  await store.paint();
  assert.equal(latestResponses.length, latestCount, "a disposed store does not start another poll");
  await store.hold("disposed", Uint8Array.of(9), null, true);
  assert.equal(states.some((state) => state?.sha === "disposed"), false);
}

// Polling the same rendering again updates metadata without transferring the
// PDF a second time.
{
  const timer = timerHarness();
  let renderingRequests = 0;
  let renderedSha = null;
  const delivered = [];
  const store = createRenderingStore({
    api: {
      latest: async () => ({ ok: true, json: async () => ({ sha: "same", current: false }) }),
      rendering: async () => {
        renderingRequests += 1;
        return { ok: true, arrayBuffer: async () => Uint8Array.of(3).buffer };
      },
      putRendering: async () => ({ ok: true }),
    },
    getViewing: () => null,
    getSourceGeneration: () => 0,
    getNavigationGeneration: () => 0,
    getRenderedSha: () => renderedSha,
    deliver: (payload) => {
      renderedSha = payload.sha;
      delivered.push(payload);
    },
    onRendering: () => {},
    onMissing: () => {},
    schedulePreview: () => {},
    setTimer: timer.setTimer,
    clearTimer: timer.clearTimer,
  });
  await store.paint();
  await store.paint();
  assert.equal(renderingRequests, 1, "a same-SHA poll does not fetch the PDF again");
  assert.equal(delivered.length, 1);
  assert.equal(timer.timers.length, 2, "each poll schedules the next metadata check");
  assert.equal(timer.timers[0].cleared, true, "the newer poll replaces the old timer");
  store.dispose();
}

// A rendering fetched before iframe readiness is retained by the frame
// controller and replayed after the frame announces that it is ready. The
// store and frame are tested together because the handoff is where an early
// fetch used to disappear.
{
  const sent = [];
  const timer = timerHarness();
  const frame = createFramePreview({
    slug: "doc",
    getDocsOrigin: () => "https://docs.test",
    framePath: () => "pdf",
    api: { frame: async () => ({ ok: true, json: async () => ({}) }) },
    setSource: () => {},
    send: (message) => sent.push(message),
  });
  frame.navigate();
  const store = createRenderingStore({
    api: {
      latest: async () => ({ ok: true, json: async () => null }),
      rendering: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(7, 8).buffer }),
      putRendering: async () => ({ ok: true }),
    },
    getViewing: () => ({ sha: "A", at: "a" }),
    getSourceGeneration: () => 0,
    getNavigationGeneration: () => 0,
    getPreview: () => frame.preview(),
    deliver: (payload) => frame.publish(payload),
    onRendering: () => {},
    onMissing: () => {},
    schedulePreview: () => {},
    setTimer: timer.setTimer,
    clearTimer: timer.clearTimer,
  });
  await store.paint();
  assert.equal(sent.length, 0, "a PDF fetched before readiness waits for replay");
  assert.deepEqual([...frame.preview().bytes], [7, 8]);
  frame.markReady();
  assert.equal(frame.replay(), true);
  assert.equal(sent.length, 1);
  new Uint8Array(sent[0].pdf)[0] = 99;
  assert.equal(frame.preview().bytes[0], 7, "replay transfers a fresh PDF buffer");
  store.dispose();
}

// A missing historical PDF clears the old pages and records an honest missing
// rendering. A later poll may report the same miss, but it does not reload the
// frame after the old pages have already been cleared.
{
  let navigations = 0;
  let renderedSha = "live";
  let rendering = null;
  const timer = timerHarness();
  const frame = createFramePreview({
    slug: "doc",
    getDocsOrigin: () => "https://docs.test",
    framePath: () => "pdf",
    api: { frame: async () => ({ ok: true, json: async () => ({}) }) },
    setSource: () => {},
    send: () => {},
  });
  frame.navigate();
  frame.markReady();
  frame.publish({ kind: "pdf", sha: "live", bytes: Uint8Array.of(1) });
  const store = createRenderingStore({
    api: {
      latest: async () => ({ ok: true, json: async () => null }),
      rendering: async () => ({ ok: false, arrayBuffer: async () => null }),
      putRendering: async () => ({ ok: true }),
    },
    getViewing: () => ({ sha: "checkpoint", at: "today" }),
    getSourceGeneration: () => 0,
    getNavigationGeneration: () => 0,
    getRenderedSha: () => renderedSha,
    getPreview: () => frame.preview(),
    deliver: (payload) => frame.publish(payload),
    onRendering: (value) => (rendering = value),
    onMissing: (value) => {
      const hadPages = Boolean(frame.preview() || renderedSha);
      rendering = value;
      frame.clear();
      renderedSha = null;
      if (hadPages) {
        navigations += 1;
        frame.navigate(true);
      }
    },
    schedulePreview: () => {},
    setTimer: timer.setTimer,
    clearTimer: timer.clearTimer,
  });
  await store.paint();
  assert.equal(rendering.missing, true);
  assert.equal(frame.preview(), null);
  assert.equal(navigations, 1);
  await store.paint();
  assert.equal(navigations, 1, "a repeated missing poll does not reload the empty frame");
  store.dispose();
}

// The first successful compile is stored immediately when the server says
// that no rendering exists. Later compiles wait for the quiet delay, and an
// edit while that first lookup is in flight invalidates the held bytes.
{
  const run = async ({ latest, current = true, editDuringAsk = false, known = false }) => {
    const timer = timerHarness();
    const stored = [];
    const states = [];
    let asked = 0;
    let source = 0;
    const store = createRenderingStore({
      api: {
        latest: async () => {
          asked += 1;
          if (editDuringAsk) source += 1;
          return { ok: true, json: async () => latest };
        },
        rendering: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(0).buffer }),
        putRendering: async (name, suffix, bytes) => {
          stored.push({ name, suffix, bytes });
          return { ok: true };
        },
      },
      getViewing: () => null,
      getSourceGeneration: () => source,
      getNavigationGeneration: () => 0,
      getRenderedSha: () => (known ? "sha0" : null),
      deliver: () => {},
      onRendering: (value) => states.push(value),
      onMissing: () => {},
      schedulePreview: () => {},
      setTimer: timer.setTimer,
      clearTimer: timer.clearTimer,
    });
    if (known) await store.paint();
    await store.hold("sha1", Uint8Array.of(1, 2), null, current);
    await new Promise(setImmediate);
    return { asked, stored, states, timers: timer.timers, store };
  };

  const first = await run({ latest: { live: "sha1" } });
  assert.equal(first.stored.length, 1, "the first rendering is stored at once");
  assert.equal(first.stored[0].name, "sha1");
  assert.deepEqual(first.timers, []);

  const later = await run({ latest: { live: "sha1", sha: "sha0" } });
  assert.equal(later.stored.length, 0, "a document with a rendering waits for quiet time");
  assert.deepEqual(later.timers.map((timer) => timer.ms), [60_000]);
  later.timers[0].fn();
  await new Promise(setImmediate);
  assert.equal(later.stored.length, 1, "quiet time stores the held rendering");

  const known = await run({ latest: { sha: "sha0" }, known: true });
  assert.equal(known.asked, 1, "a known rendering is checked once, before the hold");
  assert.deepEqual(known.timers.map((timer) => timer.ms), [30_000, 60_000]);

  const stale = await run({ latest: { live: "sha1" }, current: false });
  assert.equal(stale.stored.length, 0, "a non-current compile is held for quiet time");
  assert.deepEqual(stale.timers.map((timer) => timer.ms), [60_000]);

  const edited = await run({ latest: { live: "sha1" }, editDuringAsk: true });
  assert.equal(edited.stored.length, 0, "an edit while asking drops the held bytes");
  assert.deepEqual(edited.timers, []);
  for (const result of [first, later, known, stale, edited]) result.store.dispose();
}

// Reset cancels both kinds of delayed work and invalidates a latest-rendering
// probe that was already in flight.
{
  const timer = timerHarness();
  const pending = deferred();
  const stored = [];
  let latestCalls = 0;
  let source = 0;
  let states = [];
  const store = createRenderingStore({
    api: {
      latest: async () => {
        latestCalls += 1;
        return pending.promise;
      },
      rendering: async () => ({ ok: true, arrayBuffer: async () => Uint8Array.of(1).buffer }),
      putRendering: async (name) => {
        stored.push(name);
        return { ok: true };
      },
    },
    getViewing: () => null,
    getSourceGeneration: () => source,
    getNavigationGeneration: () => 0,
    deliver: () => {},
    onRendering: (value) => { states.push(value); },
    onMissing: () => {},
    schedulePreview: () => {},
    setTimer: timer.setTimer,
    clearTimer: timer.clearTimer,
  });
  store.schedulePoll();
  const probing = store.hold("probe", Uint8Array.of(4), null, true);
  await Promise.resolve();
  assert.equal(latestCalls, 1);
  assert.equal(timer.timers.length, 1, "the in-flight probe has not scheduled quiet work yet");
  store.reset();
  pending.resolve({ ok: true, json: async () => ({ live: "probe" }) });
  await probing;
  await new Promise(setImmediate);
  assert.equal(timer.timers[0].cleared, true, "reset cancels a scheduled poll");
  assert.equal(stored.length, 0, "reset drops bytes from an obsolete probe");
  assert.equal(states.at(-1), null, "reset clears rendering metadata");
  store.dispose();
}

// If the main rendering PUT causes teardown, the optional synctex PUT must
// not continue after the controller has been disposed.
{
  const puts = [];
  let store;
  store = createRenderingStore({
    api: {
      latest: async () => ({ ok: true, json: async () => null }),
      rendering: async () => ({ ok: false }),
      putRendering: async (_name, suffix) => {
        puts.push(suffix);
        return { ok: true };
      },
    },
    getViewing: () => null,
    getSourceGeneration: () => 0,
    getNavigationGeneration: () => 0,
    deliver: () => {},
    onRendering: () => store.dispose(),
    onMissing: () => {},
    schedulePreview: () => {},
  });
  await store.store("sha", Uint8Array.of(1), Uint8Array.of(2));
  assert.deepEqual(puts, [""], "teardown after the main PUT suppresses synctex");
}

console.log("reader-preview: all checks passed");
