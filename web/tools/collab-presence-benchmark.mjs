// Run with node web/tools/collab-presence-benchmark.mjs [checkout-root].
// Counts actual protocol frames for a burst; does not measure browser latency.
//
// What it is watching for is a caret that sends a frame per keystroke. Presence
// is throttled, so a hundred local moves should leave as a handful of frames,
// and somebody else's caret arriving should not echo back out at all.
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { EphemeralStore } from "loro-crdt";

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
const peer = new EphemeralStore(30000);
try {
  await session.start({});
  frames.length = 0;
  for (let head = 0; head < 100; head++) {
    session.ephemeral.set("cursor", { head });
  }
  for (const [id, callback] of [...timers]) { timers.delete(id); callback(); }
  const burstFrames = frames.filter((frame) => frame.type === "doc-presence").length;
  frames.length = 0;
  peer.set("cursor", { head: 42 });
  session.applyPresence(Buffer.from(peer.encodeAll()).toString("base64"));
  console.log(JSON.stringify({ local_changes: 100, burst_frames: burstFrames,
    remote_echo_frames: frames.filter((frame) => frame.type === "doc-presence").length }));
} finally {
  session.leave();
  peer.destroy?.();
}
