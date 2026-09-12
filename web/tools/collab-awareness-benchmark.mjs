// Run with node web/tools/collab-awareness-benchmark.mjs [checkout-root].
// Counts actual protocol frames for a burst; does not measure browser latency.
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import * as Y from "yjs";
import { Awareness, encodeAwarenessUpdate } from "y-protocols/awareness.js";

const root = resolve(process.argv[2] || new URL("../..", import.meta.url).pathname);
const { join } = await import(pathToFileURL(`${root}/web/src/lib/collab.js`));
const frames = [];
const timers = new Map();
let timerId = 0;
const session = join({
  send: (frame) => frames.push(frame),
  setTimer: (callback) => { timers.set(++timerId, callback); return timerId; },
  clearTimer: (id) => timers.delete(id),
});
const peerDoc = new Y.Doc();
const peer = new Awareness(peerDoc);
try {
  await session.start({});
  frames.length = 0;
  for (let head = 0; head < 100; head++) {
    session.awareness.setLocalStateField("cursor", { head });
  }
  for (const [id, callback] of [...timers]) { timers.delete(id); callback(); }
  const burstFrames = frames.filter((frame) => frame.type === "y-awareness").length;
  frames.length = 0;
  peer.setLocalStateField("cursor", { head: 42 });
  const update = encodeAwarenessUpdate(peer, [peerDoc.clientID]);
  session.applyAwareness(Buffer.from(update).toString("base64"));
  console.log(JSON.stringify({ local_changes: 100, burst_frames: burstFrames,
    remote_echo_frames: frames.filter((frame) => frame.type === "y-awareness").length }));
} finally {
  session.leave();
  peer.destroy();
  peerDoc.destroy();
}
