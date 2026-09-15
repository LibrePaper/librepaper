// The room socket's liveness, backoff and parsing, checked against a fake
// WebSocket rather than a server. What is being proved here is what the page
// does when the network misbehaves in the ways that do not announce
// themselves.
import assert from "node:assert/strict";

const REAL = {
  WebSocket: globalThis.WebSocket,
  location: globalThis.location,
  document: globalThis.document,
  setTimeout: globalThis.setTimeout,
  setInterval: globalThis.setInterval,
};

// A clock the test drives, so a twenty-five second heartbeat costs no time.
function clock() {
  let now = 0;
  let next = 1;
  const timers = new Map();
  const api = {
    setTimeout: (fn, ms = 0) => { const id = next++; timers.set(id, { fn, at: now + ms, every: 0 }); return id; },
    setInterval: (fn, ms = 1) => { const id = next++; timers.set(id, { fn, at: now + ms, every: ms }); return id; },
    clearTimeout: (id) => timers.delete(id),
    clearInterval: (id) => timers.delete(id),
    pending: () => timers.size,
    advance(ms) {
      const until = now + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, t]) => t.at <= until).sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        const [id, timer] = due;
        now = timer.at;
        if (timer.every) timer.at = now + timer.every;
        else timers.delete(id);
        timer.fn();
      }
      now = until;
    },
  };
  return api;
}

const sockets = [];
class FakeSocket {
  static OPEN = 1;
  static CLOSED = 3;
  constructor(url) {
    this.url = url;
    this.readyState = FakeSocket.OPEN;
    this.sent = [];
    this.closed = false;
    sockets.push(this);
  }
  send(text) { this.sent.push(JSON.parse(text)); }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.readyState = FakeSocket.CLOSED;
    this.onclose?.();
  }
  open() { this.onopen?.(); }
  deliver(value) { this.onmessage?.({ data: typeof value === "string" ? value : JSON.stringify(value) }); }
}

const listeners = new Map();
function install(time) {
  sockets.length = 0;
  listeners.clear();
  globalThis.WebSocket = FakeSocket;
  globalThis.location = { protocol: "https:", host: "example.test" };
  globalThis.document = {
    visibilityState: "visible",
    addEventListener: (name, fn) => listeners.set(`doc:${name}`, fn),
    removeEventListener: (name) => listeners.delete(`doc:${name}`),
  };
  globalThis.setTimeout = time.setTimeout;
  globalThis.clearTimeout = time.clearTimeout;
  globalThis.setInterval = time.setInterval;
  globalThis.clearInterval = time.clearInterval;
  globalThis.addEventListener = (name, fn) => listeners.set(name, fn);
  globalThis.removeEventListener = (name) => listeners.delete(name);
}
function restore() { Object.assign(globalThis, REAL); }

const { openRoom } = await import("../../src/lib/room.js");

// A socket that stops delivering without ever closing is the failure that
// does not announce itself. Nothing in the old client noticed it.
{
  const time = clock();
  install(time);
  const room = openRoom("paper", { onMessage: () => {}, onConnected: () => {} });
  sockets[0].open();

  time.advance(25_000);
  assert.deepEqual(sockets[0].sent.at(-1), { type: "ping" }, "the socket is asked whether it is alive");
  assert.equal(sockets[0].closed, false, "an unanswered ping is given time to be answered");

  time.advance(10_001);
  assert.equal(sockets[0].closed, true, "an unanswered ping closes the socket, which starts the reconnect");

  room.close();
  restore();
}

// A pong is the answer to that question and nothing more: it is never handed
// to the reader as a room event.
{
  const time = clock();
  install(time);
  const heard = [];
  const room = openRoom("paper", { onMessage: (message) => heard.push(message), onConnected: () => {} });
  sockets[0].open();

  time.advance(25_000);
  sockets[0].deliver({ type: "pong" });
  time.advance(10_001);
  assert.equal(sockets[0].closed, false, "an answered ping leaves the socket alone");
  assert.deepEqual(heard, [], "a pong is not an event");

  room.close();
  restore();
}

// A frame this page cannot parse must not throw inside an event handler,
// where nothing is catching.
{
  const time = clock();
  install(time);
  const heard = [];
  const room = openRoom("paper", { onMessage: (message) => heard.push(message), onConnected: () => {} });
  sockets[0].open();
  assert.doesNotThrow(() => sockets[0].deliver("{ not json"));
  sockets[0].deliver({ type: "comment" });
  assert.deepEqual(heard, [{ type: "comment" }], "the socket keeps working after a bad frame");
  room.close();
  restore();
}

// Every client of a server that restarted holds the same backoff. Without
// jitter they all come back at the same instant, in waves.
{
  const delays = new Set();
  for (let attempt = 0; attempt < 40; attempt++) {
    const time = clock();
    install(time);
    const room = openRoom("paper", { onMessage: () => {}, onConnected: () => {} });
    sockets[0].open();
    sockets[0].close();
    // The reconnect is the one pending timeout; find when it fires.
    let fired = -1;
    for (let ms = 1; ms <= 2000 && fired < 0; ms++) {
      time.advance(1);
      if (sockets.length > 1) fired = ms;
    }
    delays.add(fired);
    room.close();
    restore();
  }
  assert.ok(delays.size > 5, `reconnect delay is jittered (saw ${delays.size} distinct delays in 40 drops)`);
}

// A laptop that wakes does not sit out a backoff that was scheduled while it
// was asleep.
{
  const time = clock();
  install(time);
  const room = openRoom("paper", { onMessage: () => {}, onConnected: () => {} });
  sockets[0].open();
  sockets[0].close();
  assert.equal(sockets.length, 1, "a reconnect is pending, not yet made");
  listeners.get("online")?.();
  assert.equal(sockets.length, 2, "coming back online reconnects at once");
  room.close();
  restore();
}

// Closing takes everything off: no heartbeat left running, no listener left
// on the window holding a closed room alive.
{
  const time = clock();
  install(time);
  const room = openRoom("paper", { onMessage: () => {}, onConnected: () => {} });
  sockets[0].open();
  room.close();
  assert.equal(time.pending(), 0, "no timer outlives the room");
  assert.equal(listeners.has("online"), false, "no window listener outlives the room");
  restore();
}

console.log("room: heartbeat, pong handling, bad frames, jittered backoff, wake and teardown passed");
