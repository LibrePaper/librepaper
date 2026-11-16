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
// `open()` already carries this session's own (empty) vector, base64 and
// all, which is exactly what a `doc-state` reply names as `vector`: this
// test is about presence, not reconciliation, so its own opening vector is
// as good a "the server and I agree" answer as any other.
const { vector: emptyVector } = session.open();
let left = false;
try {
  // Joining announces the current local state once. A burst of 100 cursor
  // changes then produces one frame, carrying the final cursor.
  await session.start({ protocol: "librepaper.room.v3", vector: emptyVector, updates: [] });
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
  latest.destroy();

  const peer = new EphemeralStore(30000);
  peer.set("cursor", { head: 99 });
  const beforeRemote = sent.length;
  // Feed a real remote update through the public path. It must update local
  // presence without being relayed back to the room.
  const remote = encode(peer.encode("cursor"));
  session.applyPresence(remote);
  assert.equal(sent.length, beforeRemote, "remote presence is not echoed");
  peer.destroy();

  // A reconnect gets the current state again because the room only relays
  // presence while a socket is alive. A pending update is retained after
  // disconnect and sent once, rather than being left on an old timer.
  session.ephemeral.set("cursor", { head: 100 });
  assert.equal(timers.size, 1, "a new cursor update schedules one timer");
  session.disconnected();
  assert.equal(timers.size, 0, "disconnect clears the pending throttle timer");
  const beforeReconnect = presenceFrames(sent).length;
  await session.start({ protocol: "librepaper.room.v3", vector: emptyVector, updates: [] });
  assert.equal(presenceFrames(sent).length, beforeReconnect + 1, "reconnect reannounces local presence");
  const beforeRepeatedStart = presenceFrames(sent).length;
  await session.start({ protocol: "librepaper.room.v3", vector: emptyVector, updates: [] });
  assert.equal(presenceFrames(sent).length, beforeRepeatedStart, "repeated sync start does not duplicate presence");

  // A local departure bypasses the delay and leaves no timer behind. The
  // pending cursor update is superseded by the removal state.
  session.ephemeral.set("cursor", { head: 100 });
  const beforeLeave = presenceFrames(sent).length;
  // Deletion triggers immediate removal send before the scheduled update
  session.ephemeral.delete("cursor");
  assert.equal(presenceFrames(sent).length, beforeLeave + 1, "deletion is immediate");
  assert.equal(timers.size, 0, "deletion cancels pending throttle");

  session.leave();
  left = true;
  assert.equal(timers.size, 0, "destroy clears any remaining presence timer");
  const afterLeave = sent.length;
  runTimers();
  assert.equal(sent.length, afterLeave, "destroy prevents a delayed presence send");
} finally {
  if (!left) session.leave();
}

// The heartbeat must rewrite this browser's record before peers expire it, and
// stop with the socket or the session.
{
  const clock = fakeClock();
  const sent = [];
  const session = join({
    send: (message) => sent.push(message),
    mayEdit: true,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    setRepeat: clock.setRepeat,
    clearRepeat: clock.clearRepeat,
  });
  const { vector } = session.open();
  await session.start({ protocol: "librepaper.room.v3", vector, updates: [] });
  assert.equal(clock.repeats.size, 1, "joining registers one repeating timer");
  assert.deepEqual(clock.repeatDelays, [15000], "the heartbeat runs every 15 s");

  sent.length = 0;
  [...clock.repeats.values()][0]();
  clock.runTimers();
  const beats = presenceFrames(sent);
  assert.equal(beats.length, 1, "a heartbeat sends one presence frame");
  const probe = new EphemeralStore(30000);
  probe.apply(decode(beats[0].update));
  assert.ok(
    probe.getAllStates()[`user:${session.localPresence().tab}`],
    "the heartbeat frame carries this session's own record",
  );
  probe.destroy();
  session.leave();
  assert.equal(clock.repeats.size, 0, "leave clears the heartbeat");
}

{
  const clock = fakeClock();
  const session = join({
    send: () => {},
    mayEdit: true,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    setRepeat: clock.setRepeat,
    clearRepeat: clock.clearRepeat,
  });
  const { vector } = session.open();
  await session.start({ protocol: "librepaper.room.v3", vector, updates: [] });
  assert.equal(clock.repeats.size, 1);
  session.disconnected();
  assert.equal(clock.repeats.size, 0, "disconnect clears the heartbeat");
  await session.start({ protocol: "librepaper.room.v3", vector, updates: [] });
  assert.equal(clock.repeats.size, 1, "rejoining restarts the heartbeat");
  session.leave();
  assert.equal(clock.repeats.size, 0, "leave clears the heartbeat after a rejoin");
}

// Somebody who has just arrived has heard from nobody here, so each person
// already present answers the first record they see from them, once.
{
  const clock = fakeClock();
  const sent = [];
  const session = join({
    send: (message) => sent.push(message),
    mayEdit: true,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    setRepeat: clock.setRepeat,
    clearRepeat: clock.clearRepeat,
  });
  const { vector } = session.open();
  await session.start({ protocol: "librepaper.room.v3", vector, updates: [] });
  sent.length = 0;

  const other = new EphemeralStore(30000);
  other.set("user:other", { name: "Grace", tab: "other" });
  session.applyPresence(encode(other.encode("user:other")));
  const answers = presenceFrames(sent);
  assert.equal(answers.length, 1, "a newcomer's first record is answered immediately");
  const probe = new EphemeralStore(30000);
  probe.apply(decode(answers[0].update));
  assert.ok(
    probe.getAllStates()[`user:${session.localPresence().tab}`],
    "the answer carries this session's own record",
  );
  probe.destroy();

  other.set("user:other", { name: "Grace", tab: "other", file: "notes" });
  sent.length = 0;
  session.applyPresence(encode(other.encode("user:other")));
  assert.equal(presenceFrames(sent).length, 0, "an update from a known key is not answered");

  other.destroy();
  session.leave();
}

function fakeClock() {
  const timers = new Map();
  const repeats = new Map();
  const repeatDelays = [];
  let nextId = 0;
  return {
    repeats,
    repeatDelays,
    setTimer(callback, delay) {
      assert.equal(delay, 100);
      const id = ++nextId;
      timers.set(id, callback);
      return id;
    },
    clearTimer(id) {
      timers.delete(id);
    },
    runTimers() {
      for (const [id, callback] of [...timers]) {
        timers.delete(id);
        callback();
      }
    },
    setRepeat(callback, delay) {
      repeatDelays.push(delay);
      const id = ++nextId;
      repeats.set(id, callback);
      return id;
    },
    clearRepeat(id) {
      repeats.delete(id);
    },
  };
}

function decode(text) {
  return Uint8Array.from(atob(text), (character) => character.charCodeAt(0));
}

console.log("collab-awareness: throttled local bursts, remote filtering and immediate removal passed (ported to Loro)");
