import assert from "node:assert/strict";
import { createFrameMessageReceiver, validateFrameMessage } from "../../src/lib/frame-messages.js";

const accepted = (message) => validateFrameMessage({ librepaper: true, ...message });
const rejected = (message) => assert.equal(accepted(message), null, JSON.stringify(message).slice(0, 180));

// Shapes emitted by the document agent and PDF viewer are accepted and
// normalized to the small vocabulary the app consumes.
assert.deepEqual(accepted({ type: "ready", text: "A paragraph." }), { type: "ready", text: "A paragraph." });
assert.deepEqual(accepted({ type: "selection", selector: { exact: "selected text", prefix: "before", suffix: "after", position: 12, extra: "discard me" } }), {
  type: "selection", selector: { exact: "selected text", prefix: "before", suffix: "after", position: 12 },
});
assert.deepEqual(accepted({ type: "selection", selector: { exact: "selected text", prefix: "before", suffix: "after", position: 12 }, rect: { top: 0, left: 2, right: 4, bottom: 6 } }), {
  type: "selection", selector: { exact: "selected text", prefix: "before", suffix: "after", position: 12 }, rect: { top: 0, left: 2, right: 4, bottom: 6 },
});
assert.deepEqual(accepted({ type: "selection", selector: null }), { type: "selection", selector: null });
assert.deepEqual(accepted({ type: "caret", offset: 9, pdf: { page: 2, x: 0, y: 12 } }), { type: "caret", offset: 9, pdf: { page: 2, x: 0, y: 12 } });
assert.deepEqual(accepted({ type: "pdf-caret", pdf: { page: 1, x: 10, y: 20 } }), { type: "pdf-caret", pdf: { page: 1, x: 10, y: 20 } });
assert.deepEqual(accepted({ type: "viewer-state", drawn: true, mode: "page-fit", scale: 1.25 }), { type: "viewer-state", drawn: true, mode: "page-fit", scale: 1.25 });
assert.deepEqual(accepted({ type: "viewer-state", drawn: false, mode: "1.5", scale: null }), { type: "viewer-state", drawn: false, mode: "1.5", scale: null });
for (const message of [
  { type: "annotate" }, { type: "disarm" }, { type: "focus", id: "mark-1" },
  { type: "reading-position", start: 4 }, { type: "suggestion-review", id: "mark-1", reason: "overlap" },
]) assert.ok(accepted(message), `${message.type} is an emitted frame message`);

// Reject malformed objects, privileged/unknown vocabulary, oversize strings,
// non-finite coordinates and unsafe integer offsets at the boundary.
for (const message of [
  null, {}, { type: "admin-command", action: "read-session" }, { type: "ready" },
  { type: "ready", text: "x".repeat(16_000_001) }, { type: "ready", text: 4 },
  { type: "selection", selector: { exact: "" } }, { type: "selection", selector: { exact: "x", position: 1.5 } },
  { type: "selection", selector: { exact: "x", prefix: "x".repeat(4097) } },
  { type: "selection", selector: { exact: "x", prefix: "", suffix: "" }, rect: { top: 0, left: 0, right: 1, bottom: Infinity } },
  { type: "caret", offset: Number.MAX_SAFE_INTEGER + 1 }, { type: "caret", offset: -1 },
  { type: "pdf-caret", pdf: { page: 1, x: NaN, y: 1 } },
  { type: "viewer-state", drawn: true, mode: "page-fit", scale: Infinity },
  { type: "viewer-state", drawn: "yes", mode: "page-fit", scale: 1 },
  { type: "viewer-state", drawn: true, mode: "10.1", scale: 1 },
  { type: "focus", id: "x".repeat(257) }, { type: "reading-position", start: 1.25 },
  { type: "suggestion-review", id: "x", reason: "execute" },
]) rejected(message);

// A long ready message costs more bucket units than a short one: ten 2 MB
// repaints exhaust the 32 unit budget although the message count stays low.
const weightedMessages = [];
const weightedWindow = {};
const weightedReceiver = createFrameMessageReceiver({
  getFrame: () => ({ contentWindow: weightedWindow }),
  getSrc: () => "https://docs.example/raw/weighted",
  getDocsOrigin: () => "https://docs.example",
  onmessage: (message) => weightedMessages.push(message),
  now: () => 0,
  setTimer: () => 1,
  clearTimer: () => {},
});
for (let i = 0; i < 11; i++) weightedReceiver.receive({
  source: weightedWindow, origin: "https://docs.example",
  data: { librepaper: true, type: "ready", text: "x".repeat(2_000_001) },
});
assert.equal(weightedMessages.length, 10, "ten 3-unit payloads consume 30 of 32 units, so the eleventh is queued");
weightedReceiver.dispose();

let time = 0;
let timerId = 0;
const timers = new Map();
function setTimer(callback, delay) {
  const id = ++timerId;
  timers.set(id, { callback, due: time + delay });
  return id;
}
function clearTimer(id) { timers.delete(id); }
function advance(ms) {
  const target = time + ms;
  while (true) {
    const next = [...timers.entries()].sort((a, b) => a[1].due - b[1].due)[0];
    if (!next || next[1].due > target) break;
    timers.delete(next[0]);
    time = next[1].due;
    next[1].callback();
  }
  time = target;
}

// Mixed queues must share one correctly rescheduled timer. An expensive ready
// repaint initially needs eight seconds of tokens; an exhausted selection
// bucket is due in about 83ms; replacing the queued repaint with a one-unit
// update moves its own deadline to 500ms without delaying that selection.
let mixedTime = 0;
let mixedTimerId = 0;
const mixedTimers = new Map();
function setMixedTimer(callback, delay) {
  const id = ++mixedTimerId;
  mixedTimers.set(id, { callback, due: mixedTime + delay });
  return id;
}
function clearMixedTimer(id) { mixedTimers.delete(id); }
function advanceMixed(ms) {
  const target = mixedTime + ms;
  while (true) {
    const next = [...mixedTimers.entries()].sort((a, b) => a[1].due - b[1].due)[0];
    if (!next || next[1].due > target) break;
    mixedTimers.delete(next[0]);
    mixedTime = next[1].due;
    next[1].callback();
  }
  mixedTime = target;
}
const mixedWindow = {};
const mixedOutput = [];
const mixedReceiver = createFrameMessageReceiver({
  getFrame: () => ({ contentWindow: mixedWindow }),
  getSrc: () => "https://docs.example/raw/mixed",
  getDocsOrigin: () => "https://docs.example",
  onmessage: (message) => mixedOutput.push(message),
  now: () => mixedTime,
  setTimer: setMixedTimer,
  clearTimer: clearMixedTimer,
});
const mixedEvent = (data) => mixedReceiver.receive({ source: mixedWindow, origin: "https://docs.example", data: { librepaper: true, ...data } });
const expensiveReady = "x".repeat(15_000_001);
mixedEvent({ type: "ready", text: expensiveReady });
mixedEvent({ type: "ready", text: expensiveReady });
const beforeMixedQueue = mixedOutput.length;
mixedEvent({ type: "ready", text: expensiveReady });
for (let i = 0; i < 48; i++) mixedEvent({ type: "selection", selector: { exact: `spent-${i}`, prefix: "", suffix: "" } });
mixedEvent({ type: "selection", selector: { exact: "eligible-in-83ms", prefix: "", suffix: "" } });
mixedEvent({ type: "ready", text: "eligible-in-500ms" });
advanceMixed(100);
assert.ok(mixedOutput.some((message) => message.selector?.exact === "eligible-in-83ms"));
assert.ok(!mixedOutput.some((message) => message.text === "eligible-in-500ms"));
advanceMixed(400);
assert.ok(mixedOutput.some((message) => message.text === "eligible-in-500ms"));
assert.ok(!mixedOutput.slice(beforeMixedQueue).some((message) => message.text === expensiveReady), "the smaller latest repaint replaced the queued expensive repaint");
mixedReceiver.dispose();

const frameWindow = {};
const frame = { contentWindow: frameWindow };
let src = "https://docs.example/raw/paper/?v=1";
const received = [];
const receiver = createFrameMessageReceiver({
  getFrame: () => frame,
  getSrc: () => src,
  getDocsOrigin: () => "https://docs.example",
  onmessage: (message) => received.push(message),
  now: () => time,
  setTimer,
  clearTimer,
});
const event = (data, source = frameWindow, origin = "https://docs.example") => receiver.receive({ data, source, origin });

// A known message is delivered only from the current frame at the trusted
// origin. Same-origin siblings, stale iframe windows and a wrong origin fail.
event({ librepaper: true, type: "ready", text: "first" });
assert.equal(received.length, 1);
event({ librepaper: true, type: "ready", text: "sibling" }, {});
event({ librepaper: true, type: "ready", text: "wrong origin" }, frameWindow, "https://evil.example");
event({ librepaper: true, type: "admin-command" });
assert.deepEqual(received.map((message) => message.text), ["first"]);

// The ready bucket is finite (32 units) and refills at 2 units/second. While
// exhausted, ready/selection retain the latest useful update and deliver it
// when a token becomes available, instead of losing the final repaint.
for (let i = 0; i < 31; i++) event({ librepaper: true, type: "ready", text: `burst-${i}` });
for (let i = 0; i < 48; i++) event({ librepaper: true, type: "selection", selector: { exact: `selection-${i}`, prefix: "", suffix: "" } });
const beforeQueue = received.length;
event({ librepaper: true, type: "ready", text: "queued-ready-1" });
event({ librepaper: true, type: "ready", text: "queued-ready-latest" });
event({ librepaper: true, type: "selection", selector: { exact: "queued-selection-1", prefix: "", suffix: "" } });
event({ librepaper: true, type: "selection", selector: { exact: "queued-selection-latest", prefix: "", suffix: "" } });
assert.equal(received.length, beforeQueue);
advance(500);
assert.ok(received.some((message) => message.text === "queued-ready-latest"));
assert.ok(received.some((message) => message.selector?.exact === "queued-selection-latest"));
assert.ok(!received.some((message) => message.text === "queued-ready-1" || message.selector?.exact === "queued-selection-1"));

// A new repaint that arrives after a token has refilled supersedes an older
// queued repaint and is delivered directly; the stale timer cannot follow it.
receiver.reset();
for (let i = 0; i < 32; i++) event({ librepaper: true, type: "ready", text: `fill-before-newer-${i}` });
event({ librepaper: true, type: "ready", text: "stale-queued" });
time += 500;
event({ librepaper: true, type: "ready", text: "newer-in-budget" });
assert.equal(received.at(-1).text, "newer-in-budget");
advance(500);
assert.ok(!received.some((message) => message.text === "stale-queued"));

// The iframe load hook cancels pending data without restoring spent tokens.
// A new small repaint after the same-URL reload waits for the normal refill.
receiver.reset();
for (let i = 0; i < 32; i++) event({ librepaper: true, type: "ready", text: `fill-before-frame-load-${i}` });
for (let i = 0; i < 48; i++) event({ librepaper: true, type: "selection", selector: { exact: `fill-selection-${i}`, prefix: "", suffix: "" } });
event({ librepaper: true, type: "ready", text: "stale-ready-before-frame-load" });
event({ librepaper: true, type: "selection", selector: { exact: "stale-selection-before-frame-load", prefix: "", suffix: "" } });
receiver.frameLoaded();
event({ librepaper: true, type: "ready", text: "fresh-ready-after-frame-load" });
assert.ok(!received.some((message) => message.text === "fresh-ready-after-frame-load"), "frameLoaded retains the depleted same-URL budget");
advance(500);
assert.ok(received.some((message) => message.text === "fresh-ready-after-frame-load"));
assert.ok(!received.some((message) => message.text === "stale-ready-before-frame-load" || message.selector?.exact === "stale-selection-before-frame-load"));

// A source URL change clears its old bucket and pending work. Explicit reset
// and dispose also cancel queued data so a previous document cannot publish
// into the next one.
receiver.reset();
for (let i = 0; i < 32; i++) event({ librepaper: true, type: "ready", text: `fill-before-reset-${i}` });
event({ librepaper: true, type: "ready", text: "stale-after-reset" });
src = "https://docs.example/raw/paper/?v=2";
event({ librepaper: true, type: "ready", text: "after-navigation-reset" });
assert.equal(received.at(-1).text, "after-navigation-reset");
for (let i = 0; i < 31; i++) event({ librepaper: true, type: "ready", text: `fill-before-explicit-reset-${i}` });
event({ librepaper: true, type: "ready", text: "stale-after-explicit-reset" });
receiver.reset();
event({ librepaper: true, type: "ready", text: "after-explicit-reset" });
assert.equal(received.at(-1).text, "after-explicit-reset");
for (let i = 0; i < 31; i++) event({ librepaper: true, type: "ready", text: `fill-before-dispose-${i}` });
event({ librepaper: true, type: "ready", text: "stale-after-dispose" });
receiver.dispose();
event({ librepaper: true, type: "ready", text: "after-dispose" });
advance(10_000);
assert.ok(!received.some((message) => message.text?.startsWith("stale-") || message.text === "after-dispose"));

console.log("frame-messages: schema boundaries, normalization, event authentication, rate limiting and refill passed");
