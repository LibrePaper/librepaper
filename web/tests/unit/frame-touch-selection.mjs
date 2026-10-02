// The iframe agent must wait until a touch selection has settled before it
// reports the annotation selector. Android may finalize the native selection
// after pointerup (or cancel a handle gesture), so exercise the shipped event
// wiring with controllable timers rather than copying its state machine.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const frame = readFileSync(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
const start = frame.indexOf("  // A single pending timer for selection capture");
const end = frame.indexOf("  // The agent is injected before </body>", start);
assert.notEqual(start, -1, "frame selection handling is present");
assert.notEqual(end, -1, "frame selection handling has a boundary");
const selectionHandling = frame.slice(start, end);
const gestureStart = selectionHandling.indexOf("  // A selection is only reported once");
assert.notEqual(gestureStart, -1, "frame touch gesture handling is present");
const gestureHandling = selectionHandling.slice(gestureStart);

function harness({ pointerEvents = true } = {}) {
  const listeners = new Map();
  const timers = new Map();
  const posts = [];
  const captures = [];
  let nextTimer = 0;
  const document = {
    addEventListener(type, listener) { listeners.set(type, listener); },
  };
  const setTimeout = (callback, delay) => {
    const id = ++nextTimer;
    timers.set(id, { callback, delay });
    return id;
  };
  const clearTimeout = (id) => timers.delete(id);
  const install = new Function(
    "document", "post", "captureSelection", "setTimeout", "clearTimeout", "PointerEvent",
    `${selectionHandling}\nreturn true;`,
  );
  install(
    document,
    (message) => posts.push(message),
    () => captures.push("capture"),
    setTimeout,
    clearTimeout,
    pointerEvents ? function PointerEvent() {} : undefined,
  );
  const installed = listeners;
  const fire = (type, event = {}) => installed.get(type)?.({ pointerType: "touch", ...event });
  const run = (delay) => {
    for (const [id, timer] of [...timers]) {
      if (timer.delay !== delay) continue;
      timers.delete(id);
      timer.callback();
    }
  };
  return { captures, fire, listeners: installed, posts, run, timers };
}

// Touch Pointer Events own modern Android gestures. The final next-task
// debounce waits for the browser's selection adjustment without suppressing
// the native Copy toolbar.
{
  const agent = harness();
  assert.ok(agent.listeners.has("pointerdown"));
  assert.ok(agent.listeners.has("pointerup"));
  assert.ok(agent.listeners.has("pointercancel"));
  agent.fire("pointerdown");
  agent.fire("mousedown"); // Android compatibility mouse event
  assert.deepEqual(agent.posts, [{ type: "selection", selector: null }]);
  agent.fire("pointerup");
  agent.fire("mouseup"); // must not replace the touch debounce with delay 0
  agent.run(0);
  agent.run(120);
  assert.deepEqual(agent.captures, ["capture"]);

  // A late selectionchange replaces the final capture with its own debounce.
  agent.fire("selectionchange");
  agent.run(120);
  assert.deepEqual(agent.captures, ["capture"]);
  agent.run(80);
  assert.deepEqual(agent.captures, ["capture", "capture"]);
}

// A canceled touch (for example, a selection-handle gesture handed back to
// Android) still settles the current native selection. Older touch-only
// engines use the same path through touchcancel.
{
  const pointerAgent = harness();
  pointerAgent.fire("pointerdown");
  pointerAgent.fire("pointercancel");
  pointerAgent.run(0);
  pointerAgent.run(120);
  assert.deepEqual(pointerAgent.captures, ["capture"]);

  const touchAgent = harness({ pointerEvents: false });
  assert.ok(touchAgent.listeners.has("touchcancel"));
  touchAgent.fire("touchstart");
  touchAgent.fire("touchcancel");
  touchAgent.run(0);
  touchAgent.run(120);
  assert.deepEqual(touchAgent.captures, ["capture"]);
}

// These listeners must leave Android's own selection and Copy UI untouched.
assert.doesNotMatch(gestureHandling, /preventDefault\s*\(/);

console.log("frame touch-selection tests passed");
