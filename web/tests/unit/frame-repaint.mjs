// The preview frame nudges its own paint after a resize or a return from away:
// an opacity just under 1 for two frames, then the previous inline value back.
// Exercise the shipped block with fake timers and frames.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const frame = readFileSync(new URL("../../src/agent/frame.js", import.meta.url), "utf8");
const start = frame.indexOf("  // Repaint nudge.");
const end = frame.indexOf("  // A document in a frame cannot be pinch-zoomed");
assert.notEqual(start, -1, "repaint block is present");
assert.notEqual(end, -1, "repaint block has a boundary");

function harness(initialOpacity = "") {
  const style = {
    opacity: initialOpacity,
    removeProperty(name) { if (name === "opacity") style.opacity = ""; },
  };
  const listeners = [];
  const timers = [];
  let frames = [];
  const run = new Function(
    "document", "addEventListener", "setTimeout", "clearTimeout", "requestAnimationFrame",
    `${frame.slice(start, end)}\nreturn { repaint };`,
  );
  const api = run(
    { documentElement: { style } },
    (type, fn) => listeners.push({ type, fn }),
    (fn) => { timers.push({ fn, live: true }); return timers.length - 1; },
    (id) => { if (timers[id]) timers[id].live = false; },
    (fn) => { frames.push(fn); },
  );
  // Run every live timer once, as a browser would after its delay.
  const flushTimers = () => {
    for (const timer of timers) {
      if (!timer.live) continue;
      timer.live = false;
      timer.fn();
    }
  };
  // Run the frame callbacks queued so far; callbacks queued while running wait
  // for the next flush, as they would for the next frame.
  const flushFrame = () => {
    const queued = frames;
    frames = [];
    for (const fn of queued) fn();
  };
  return {
    ...api, style, listeners, timers, flushTimers, flushFrame,
    liveTimers: () => timers.filter((timer) => timer.live).length,
  };
}

// A resize listener is registered.
{
  const { listeners } = harness();
  assert.ok(listeners.some((listener) => listener.type === "resize"));
}

// Repeated calls arm one timer, and the nudge lands once the timer runs.
{
  const h = harness();
  h.repaint();
  h.repaint();
  h.repaint();
  assert.equal(h.liveTimers(), 1, "debounced to one timer");
  assert.equal(h.style.opacity, "", "nothing changes before the timer");
  h.flushTimers();
  assert.equal(h.style.opacity, "0.999");
  h.flushFrame();
  assert.equal(h.style.opacity, "0.999", "still nudged after one frame");
  h.flushFrame();
  assert.equal(h.style.opacity, "", "restored after two frames");
}

// A prior inline opacity is restored, not removed.
{
  const h = harness("0.5");
  h.repaint();
  h.flushTimers();
  assert.equal(h.style.opacity, "0.999");
  h.flushFrame();
  h.flushFrame();
  assert.equal(h.style.opacity, "0.5");
}

// A repaint while a nudge is in flight still restores the original value.
{
  const h = harness("0.5");
  h.repaint();
  h.flushTimers();
  h.flushFrame();
  h.repaint();
  h.flushTimers();
  h.flushFrame();
  h.flushFrame();
  assert.equal(h.style.opacity, "0.5", "original value, not 0.999");
  // And a later nudge works again from the restored value.
  h.repaint();
  h.flushTimers();
  assert.equal(h.style.opacity, "0.999");
  h.flushFrame();
  h.flushFrame();
  assert.equal(h.style.opacity, "0.5");
}
