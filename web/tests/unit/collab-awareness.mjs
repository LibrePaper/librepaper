import assert from "node:assert/strict";
import { EphemeralStore } from "loro-crdt";
import { join, uniquePresences } from "../../src/lib/collab.js";

const encode = (bytes) => btoa(String.fromCharCode(...bytes));
const presenceFrames = (sent) => sent.filter((message) => message.type === "doc-presence");

{
  const states = new Map([
    ["1", { user: { name: "Ada", tab: "same-tab" } }],
    ["2", { user: { name: "Ada", tab: "same-tab" } }],
    ["3", { user: { name: "Ada", tab: "other-tab" } }],
    ["4", { user: { name: "Grace" } }],
  ]);
  const people = uniquePresences(states);
  assert.deepEqual(people.map(({ key }) => key), ["same-tab", "other-tab", "client:4"]);
  assert.equal(uniquePresences(states, { localTab: "same-tab" }).length, 2,
    "reconnects from this tab disappear while another tab for the same user remains");
}

const timers = new Map();
let timerId = 0;
const setTimer = (callback, delay) => {
  assert.equal(delay, 100);
  const id = ++timerId;
  timers.set(id, callback);
  return id;
};
const clearTimer = (id) => timers.delete(id);
const runTimers = () => {
  for (const [id, callback] of [...timers]) {
    timers.delete(id);
    callback();
  }
};

const sent = [];
const session = join({ send: (message) => sent.push(message), mayEdit: true, setTimer, clearTimer });
let left = false;
try {
  // Joining announces the current local state once. A burst of 100 cursor
  // changes then produces one frame, carrying the final cursor.
  await session.start({});
  sent.length = 0;
  for (let head = 0; head < 100; head++) {
    session.ephemeral.set("cursor", { head });
  }
  assert.equal(presenceFrames(sent).length, 0, "cursor burst waits for the throttle");
  assert.equal(timers.size, 1, "cursor burst uses one timer");
  runTimers();
  assert.equal(presenceFrames(sent).length, 1, "100 local updates coalesce into one frame");
  const latest = new EphemeralStore(30000);
  const cursorFrame = presenceFrames(sent)[0].update;
  latest.apply(decode(cursorFrame));
  const allStates = latest.getAllStates();
  assert.equal(allStates.cursor?.head, 99, "the frame carries the latest cursor");

  const peer = new EphemeralStore(30000);
  peer.set("cursor", { head: 99 });
  const beforeRemote = sent.length;
  // Feed a real remote update through the public path. It must update local
  // presence without being relayed back to the room.
  const remote = encode(peer.encode("cursor"));
  session.applyPresence(remote);
  assert.equal(sent.length, beforeRemote, "remote presence is not echoed");

  // A reconnect gets the current state again because the room only relays
  // presence while a socket is alive. A pending update is retained after
  // disconnect and sent once, rather than being left on an old timer.
  session.ephemeral.set("cursor", { head: 100 });
  assert.equal(timers.size, 1, "a new cursor update schedules one timer");
  session.disconnected();
  assert.equal(timers.size, 0, "disconnect clears the pending throttle timer");
  const beforeReconnect = presenceFrames(sent).length;
  await session.start({});
  assert.equal(presenceFrames(sent).length, beforeReconnect + 1, "reconnect reannounces local presence");
  const beforeRepeatedStart = presenceFrames(sent).length;
  await session.start({});
  assert.equal(presenceFrames(sent).length, beforeRepeatedStart, "repeated sync start does not duplicate presence");

  // A local departure bypasses the delay and leaves no timer behind. The
  // pending cursor update is superseded by the removal state.
  session.ephemeral.set("cursor", { head: 100 });
  const beforeLeave = presenceFrames(sent).length;
  session.leave();
  left = true;
  assert.equal(presenceFrames(sent).length, beforeLeave + 1, "local removal is immediate");
  assert.equal(timers.size, 0, "destroy clears the presence timer");
  const afterLeave = sent.length;
  runTimers();
  assert.equal(sent.length, afterLeave, "destroy prevents a delayed presence send");
} finally {
  if (!left) session.leave();
}

function decode(text) {
  return Uint8Array.from(atob(text), (character) => character.charCodeAt(0));
}

console.log("collab-awareness: throttled local bursts, remote filtering and immediate removal passed (ported to Loro)");
