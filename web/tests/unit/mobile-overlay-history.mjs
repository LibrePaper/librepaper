import assert from "node:assert/strict";
import test from "node:test";
import { createOverlayHistory } from "../../src/lib/mobile/overlay-history.js";

// A window with just enough history and listeners for the module. A real
// go() fires its popstate later, so the tests fire it by hand with popstate().
function fakeWindow() {
  const listeners = new Map();
  return {
    history: {
      pushes: [],
      goes: [],
      pushState(state, title) {
        this.pushes.push([state, title]);
      },
      go(n) {
        this.goes.push(n);
      },
    },
    addEventListener(type, fn) {
      listeners.set(type, fn);
    },
    removeEventListener(type, fn) {
      if (listeners.get(type) === fn) listeners.delete(type);
    },
    popstate() {
      listeners.get("popstate")?.({});
    },
    listeners,
  };
}

test("opening a sheet then a panel pushes one entry per level, with no URL", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const win = fakeWindow();
  const overlays = createOverlayHistory({ onpop() {}, win, settle: 50 });

  overlays.sync(2);
  t.mock.timers.tick(49);
  assert.equal(win.history.pushes.length, 0, "nothing happens before the settle delay");
  t.mock.timers.tick(1);
  assert.deepEqual(win.history.pushes, [
    [{ lpOverlay: 1 }, ""],
    [{ lpOverlay: 2 }, ""],
  ]);
  assert.deepEqual(win.history.goes, []);
  overlays.dispose();
});

test("closing and opening again within the settle delay touches no history", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const win = fakeWindow();
  const overlays = createOverlayHistory({ onpop() {}, win, settle: 50 });

  overlays.sync(1);
  t.mock.timers.tick(50);
  assert.equal(win.history.pushes.length, 1);

  // Closing the sheet and opening a panel in one gesture.
  overlays.sync(0);
  t.mock.timers.tick(20);
  overlays.sync(1);
  t.mock.timers.tick(50);
  assert.equal(win.history.pushes.length, 1, "the level stayed open, so nothing new was pushed");
  assert.deepEqual(win.history.goes, [], "nor was anything popped");

  // And from nothing open: a quick open and close leaves no entry behind.
  overlays.sync(0);
  t.mock.timers.tick(50);
  overlays.sync(1);
  overlays.sync(0);
  t.mock.timers.tick(50);
  assert.equal(win.history.pushes.length, 1);
  assert.deepEqual(win.history.goes, [-1], "the open level was popped once, when it closed");
  overlays.dispose();
});

test("a popstate the app caused itself is ignored", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const win = fakeWindow();
  const popped = [];
  const overlays = createOverlayHistory({ onpop: (depth) => popped.push(depth), win, settle: 50 });

  overlays.sync(1);
  t.mock.timers.tick(50);
  overlays.sync(0);
  t.mock.timers.tick(50);
  assert.deepEqual(win.history.goes, [-1]);

  // The popstate that go() fires comes back to us and must not close anything.
  win.popstate();
  assert.deepEqual(popped, []);
  overlays.dispose();
});

test("a user popstate closes the top level and reports the new depth", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const win = fakeWindow();
  const popped = [];
  const overlays = createOverlayHistory({ onpop: (depth) => popped.push(depth), win, settle: 50 });

  overlays.sync(2);
  t.mock.timers.tick(50);

  win.popstate();
  assert.deepEqual(popped, [1]);
  win.popstate();
  assert.deepEqual(popped, [1, 0]);

  // Nothing is left to pop, so a further Back is a real navigation.
  win.popstate();
  assert.deepEqual(popped, [1, 0]);
  assert.deepEqual(win.history.goes, [], "the user's Back never asks history to move");

  // The app now agrees with history, so opening again pushes from zero.
  overlays.sync(1);
  t.mock.timers.tick(50);
  assert.equal(win.history.pushes.length, 3);
  assert.deepEqual(win.history.pushes[2], [{ lpOverlay: 1 }, ""]);
  overlays.dispose();
});

test("dispose clears a pending sync and stops listening", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const win = fakeWindow();
  const popped = [];
  const overlays = createOverlayHistory({ onpop: (depth) => popped.push(depth), win, settle: 50 });

  overlays.sync(1);
  overlays.dispose();
  t.mock.timers.tick(100);
  assert.equal(win.history.pushes.length, 0);
  assert.equal(win.listeners.has("popstate"), false);
});
