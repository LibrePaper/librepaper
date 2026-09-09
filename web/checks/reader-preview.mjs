import assert from "node:assert/strict";
import { createFramePreview } from "../src/lib/reader/frame-preview.js";
import { createRenderingStore } from "../src/lib/reader/rendering-store.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => (resolve = done));
  return { promise, resolve };
};

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
}

// A late rendering fetch cannot overwrite a newer request, and disposal
// prevents a quiet timer from publishing bytes after the component leaves.
{
  const latestResponses = [deferred(), deferred()];
  const latest = [...latestResponses];
  const renderingBytes = { old: deferred(), new: deferred() };
  const states = [];
  const delivered = [];
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
}

console.log("reader-preview: all checks passed");
