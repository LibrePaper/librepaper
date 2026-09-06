import assert from "node:assert/strict";
import * as Y from "yjs";
import { documentPlaceFor } from "../src/lib/sync.js";
import { zip } from "../src/lib/zip.js";
import { gather, release } from "../src/lib/figures.js";
import { join } from "../src/lib/collab.js";
import { openRoom } from "../src/lib/room.js";
import { keyFor, linkFor, takeKeyFromFragment } from "../src/lib/storage.js";

// #25: malformed HTML entities are input to a best-effort matcher, never an
// exception that escapes caret synchronization.
assert.doesNotThrow(() => documentPlaceFor(
  "<p>&#x110000; &#0; &#xD800; Some sufficiently long phrase to match</p>",
  0,
  "Some sufficiently long phrase to match",
  "html",
));

// #14: both ZIP headers identify UTF-8 entry names.
const archive = new Uint8Array(await (await zip({ "é.tex": "é", "文/main.typ": "= title" })).arrayBuffer());
const flags = [];
for (let at = 0; at + 4 <= archive.length; at++) {
  const signature = new DataView(archive.buffer).getUint32(at, true);
  if (signature === 0x04034b50) flags.push(new DataView(archive.buffer).getUint16(at + 6, true));
  if (signature === 0x02014b50) flags.push(new DataView(archive.buffer).getUint16(at + 8, true));
}
assert.equal(flags.length, 4);
assert.ok(flags.every((flag) => flag & (1 << 11)));

// #19: the key remains usable for the session and copied links when storage
// access is denied.
const savedLocation = globalThis.location;
const savedHistory = globalThis.history;
const savedStorage = globalThis.localStorage;
globalThis.location = { hash: "#k=private-key", pathname: "/docs/slug", search: "", origin: "https://example.test" };
globalThis.history = { replaceState() {} };
globalThis.localStorage = {
  getItem() { throw new Error("storage denied"); },
  setItem() { throw new Error("storage denied"); },
};
try {
  assert.equal(takeKeyFromFragment("slug"), "private-key");
  assert.equal(keyFor("slug"), "private-key");
  assert.match(linkFor("slug"), /#k=private-key$/);
} finally {
  globalThis.location = savedLocation;
  globalThis.history = savedHistory;
  globalThis.localStorage = savedStorage;
}

// #13: export callers can request an all-or-nothing asset gather while the
// normal preview path still returns the successful subset.
const oldFetch = globalThis.fetch;
globalThis.fetch = async (url) => url.endsWith("/good")
  ? new Response(new Uint8Array([1, 2, 3]), { status: 200 })
  : new Response("missing", { status: 404 });
try {
  const partial = await gather("slug", { "good.png": "good", "bad.png": "bad" });
  assert.deepEqual(partial.missing, ["bad.png"]);
  assert.match(partial.urls["good.png"], /#komodoc-asset=good$/);
  await assert.rejects(
    gather("slug", { "good.png": "good", "bad.png": "bad" }, {}, { strict: true }),
    (error) => error.missing?.[0] === "bad.png",
  );
} finally {
  release();
  globalThis.fetch = oldFetch;
}

// #1: a large Yjs update is split into bounded JSON frames and retains one
// logical sequence number for the server's eventual durability ack.
const frames = [];
const session = join({ send: (message) => frames.push(message), slug: "review", mayEdit: true });
const id = session.addText("main.typ", "x".repeat(700_000));
session.setMain(id);
const start = frames.find((message) => message.type === "y-update-start");
assert.ok(start);
const chunks = frames.filter((message) => message.type === "y-update-chunk");
assert.equal(chunks.length, start.chunks);
assert.ok(chunks.every((message) => JSON.stringify(message).length < 1_048_576));
assert.equal(frames.filter((message) => message.type === "y-update-end" && message.seq === start.seq).length, 1);
// The server's vector keeps an unchanged rejoin small even for a large tree.
frames.length = 0;
await session.start({ vector: Buffer.from(Y.encodeStateVector(session.doc)).toString("base64") });
const catchup = frames.find((message) => message.type === "y-update");
assert.ok(catchup);
assert.ok(JSON.stringify(catchup).length < 1024);
assert.ok(!frames.some((message) => message.type === "y-update-start"));
let fileEvents = 0;
session.onFiles(() => fileEvents++);
session.textOf(id).insert(0, "remote edit");
assert.equal(fileEvents, 1); // #6: nested Y.Text changes invalidate the directory watcher.
session.putAsset("figure.png", "digest");
session.renameFile("figure.png", "renamed.png", "asset");
assert.deepEqual({ ...session.tree().digests }, { "renamed.png": "digest" }); // #12
session.leave();

// #26: closing during reconnect backoff cancels the pending reconnect and the
// disconnected callback.
const oldLocation = globalThis.location;
const oldWebSocket = globalThis.WebSocket;
const oldSetTimeout = globalThis.setTimeout;
const oldClearTimeout = globalThis.clearTimeout;
const timers = new Map();
let timerId = 0;
const sockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  constructor() { sockets.push(this); }
  close() { this.readyState = 3; this.onclose?.(); }
  send() {}
}
globalThis.location = { protocol: "http:", host: "localhost" };
globalThis.WebSocket = FakeWebSocket;
globalThis.setTimeout = (callback) => { const id = ++timerId; timers.set(id, callback); return id; };
globalThis.clearTimeout = (id) => { timers.delete(id); };
try {
  const room = openRoom("slug", { onMessage() {}, onConnected() {} });
  sockets[0].onclose();
  assert.ok(timers.size > 0);
  room.close();
  assert.equal(timers.size, 0);
} finally {
  globalThis.location = oldLocation;
  globalThis.WebSocket = oldWebSocket;
  globalThis.setTimeout = oldSetTimeout;
  globalThis.clearTimeout = oldClearTimeout;
}

console.log("review fixes: ok");
