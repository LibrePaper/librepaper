// A stale lazy chunk reloads the page once, and a second failure within a
// minute is left to surface so a genuinely missing file cannot loop reloads.
import assert from "node:assert/strict";
import { reloadOnStaleChunks } from "../../src/lib/stale-chunks.js";

function fakeWindow() {
  const listeners = new Map();
  const win = {
    reloads: 0,
    addEventListener(type, listener) { listeners.set(type, listener); },
    location: { reload() { win.reloads += 1; } },
    fire() {
      const event = { prevented: false, preventDefault() { this.prevented = true; } };
      listeners.get("vite:preloadError")(event);
      return event;
    },
  };
  return win;
}

function fakeStorage() {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, value),
  };
}

// First failure reloads and takes the event; a second within 60 s does not;
// after 61 s it reloads again.
{
  const win = fakeWindow();
  let time = 1_000_000;
  reloadOnStaleChunks({ win, storage: fakeStorage(), now: () => time });
  const first = win.fire();
  assert.equal(win.reloads, 1);
  assert.equal(first.prevented, true);

  time += 5_000;
  const second = win.fire();
  assert.equal(win.reloads, 1, "a second failure within 60 s does not reload");
  assert.equal(second.prevented, false, "the error is left to surface");

  time += 61_000;
  win.fire();
  assert.equal(win.reloads, 2, "after 60 s the page reloads again");
}

// With storage throwing, the first failure reloads and a second does not.
{
  const win = fakeWindow();
  const throwing = {
    getItem() { throw new Error("blocked"); },
    setItem() { throw new Error("blocked"); },
  };
  reloadOnStaleChunks({ win, storage: throwing, now: () => 0 });
  const first = win.fire();
  assert.equal(win.reloads, 1);
  assert.equal(first.prevented, true);
  win.fire();
  assert.equal(win.reloads, 1, "without storage, only one reload per page load");
}

console.log("stale-chunks: one reload on a stale lazy chunk, then the error surfaces");
