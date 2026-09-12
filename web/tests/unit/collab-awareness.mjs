import assert from "node:assert/strict";
import * as Y from "yjs";
import { Awareness, applyAwarenessUpdate, encodeAwarenessUpdate } from "y-protocols/awareness.js";
import { join } from "../../src/lib/collab.js";

const encode = (bytes) => btoa(String.fromCharCode(...bytes));
const awarenessFrames = (sent) => sent.filter((message) => message.type === "y-awareness");

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
    session.awareness.setLocalStateField("cursor", { head });
  }
  assert.equal(awarenessFrames(sent).length, 0, "cursor burst waits for the throttle");
  assert.equal(timers.size, 1, "cursor burst uses one timer");
  runTimers();
  assert.equal(awarenessFrames(sent).length, 1, "100 local updates coalesce into one frame");
  const latest = new Awareness(new Y.Doc());
  applyAwarenessUpdate(latest, decode(awarenessFrames(sent)[0].update), "remote");
  assert.equal([...latest.getStates().values()].find((state) => state.cursor)?.cursor.head, 99, "the frame carries the latest cursor");
  latest.destroy();

  const peer = new Awareness(new Y.Doc());
  peer.setLocalStateField("cursor", { head: 99 });
  const beforeRemote = sent.length;
  // Feed a real remote update through the public path. It must update local
  // awareness without being relayed back to the room.
  const remote = encodeAwarenessUpdateFor(peer);
  session.applyAwareness(remote);
  assert.equal(sent.length, beforeRemote, "remote awareness is not echoed");
  peer.destroy();

  // A reconnect gets the current state again because the room only relays
  // awareness while a socket is alive. A pending update is retained after
  // disconnect and sent once, rather than being left on an old timer.
  session.awareness.setLocalStateField("cursor", { head: 100 });
  assert.equal(timers.size, 1, "a new cursor update schedules one timer");
  session.disconnected();
  assert.equal(timers.size, 0, "disconnect clears the pending throttle timer");
  const beforeReconnect = awarenessFrames(sent).length;
  await session.start({});
  assert.equal(awarenessFrames(sent).length, beforeReconnect + 1, "reconnect reannounces local awareness");
  const beforeRepeatedStart = awarenessFrames(sent).length;
  await session.start({});
  assert.equal(awarenessFrames(sent).length, beforeRepeatedStart, "repeated sync start does not duplicate awareness");

  // A local departure bypasses the delay and leaves no timer behind. The
  // pending cursor update is superseded by the removal state.
  session.awareness.setLocalStateField("cursor", { head: 100 });
  const beforeLeave = awarenessFrames(sent).length;
  session.leave();
  left = true;
  assert.equal(awarenessFrames(sent).length, beforeLeave + 1, "local removal is immediate");
  assert.equal(timers.size, 0, "destroy clears the awareness timer");
  const afterLeave = sent.length;
  runTimers();
  assert.equal(sent.length, afterLeave, "destroy prevents a delayed awareness send");
  const gone = new Awareness(new Y.Doc());
  applyAwarenessUpdate(gone, decode(awarenessFrames(sent).at(-1).update), "remote");
  assert.equal(gone.getStates().has(session.doc.clientID), false, "the removal frame clears local awareness");
  gone.destroy();
} finally {
  if (!left) session.leave();
}

function encodeAwarenessUpdateFor(peer) {
  // Importing this alongside Awareness keeps the test independent of the
  // wire format while still checking the same update consumed by the client.
  return encode(encodeAwarenessUpdate(peer, [...peer.getStates().keys()]));
}

function decode(text) {
  return Uint8Array.from(atob(text), (character) => character.charCodeAt(0));
}

console.log("collab-awareness: throttled local bursts, remote filtering and immediate removal passed");
