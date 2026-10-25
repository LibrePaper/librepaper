// The page's side of quitting the desktop app: it answers the app's request
// once its folder is saved, and hears the app say it stays open.
import assert from "node:assert/strict";
import { createFolderSync } from "../../src/lib/reader/folder-sync.js";
import { QUIT_CANCELLED, QUIT_REQUESTED, listenForQuit } from "../../src/lib/desktop/quit.js";

const idle = () => new Promise((resolve) => setImmediate(resolve));
const request = (target, id) => {
  const event = new CustomEvent(QUIT_REQUESTED, { detail: { id }, cancelable: true });
  // False means no page claimed the request, so the app answers for it.
  return { claimed: !target.dispatchEvent(event) };
};

// A local project flushes its folder, then answers ok.
{
  const target = new EventTarget();
  const answers = [];
  const writes = [];
  const timers = { set: () => 1, clear: () => {} };
  const folder = createFolderSync({ timers, run: async () => { writes.push("written"); } });
  folder.poke();
  assert.equal(folder.pending(), true);
  const stop = listenForQuit({
    target,
    settle: () => folder.flush(),
    answer: async (body) => { answers.push(body); },
  });
  assert.equal(request(target, 7).claimed, true, "the page claims the request");
  assert.deepEqual(answers, [], "the answer waits for the flush");
  await idle();
  assert.deepEqual(writes, ["written"], "the pending write ran");
  assert.deepEqual(answers, [{ id: 7, ok: true }]);
  assert.equal(folder.pending(), false);
  stop();
  folder.dispose();
}

// A folder that could not be saved answers not ok.
{
  const target = new EventTarget();
  const answers = [];
  const folder = createFolderSync({
    timers: { set: () => 1, clear: () => {} },
    run: async () => { throw new Error("disk full"); },
    onError: () => {},
  });
  folder.poke();
  listenForQuit({ target, settle: () => folder.flush(), answer: async (body) => { answers.push(body); } });
  request(target, 8);
  await idle();
  assert.deepEqual(answers, [{ id: 8, ok: false }]);
  folder.dispose();
}

// A flush that throws or resolves false is also not ok.
{
  const target = new EventTarget();
  const answers = [];
  let outcome = "throws";
  listenForQuit({
    target,
    settle: async () => { if (outcome === "throws") throw new Error("gone"); return false; },
    answer: async (body) => { answers.push(body); },
  });
  request(target, 1);
  await idle();
  outcome = "false";
  request(target, 2);
  await idle();
  assert.deepEqual(answers, [{ id: 1, ok: false }, { id: 2, ok: false }]);
}

// A web project has nothing to flush and answers ok at once.
{
  const target = new EventTarget();
  const answers = [];
  listenForQuit({ target, answer: async (body) => { answers.push(body); } });
  assert.equal(request(target, 3).claimed, true);
  await idle();
  assert.deepEqual(answers, [{ id: 3, ok: true }]);
}

// The cancellation reaches the page, and a stopped listener hears nothing.
{
  const target = new EventTarget();
  const answers = [];
  let stays = 0;
  const stop = listenForQuit({
    target,
    answer: async (body) => { answers.push(body); },
    cancelled: () => { stays += 1; },
  });
  target.dispatchEvent(new CustomEvent(QUIT_CANCELLED, { detail: { id: 4 } }));
  assert.equal(stays, 1);
  stop();
  target.dispatchEvent(new CustomEvent(QUIT_CANCELLED, { detail: { id: 5 } }));
  assert.equal(request(target, 5).claimed, false, "an unsubscribed page does not claim");
  await idle();
  assert.equal(stays, 1);
  assert.deepEqual(answers, []);
}

// An answer that cannot be sent does not escape as an unhandled rejection.
{
  const target = new EventTarget();
  listenForQuit({ target, answer: async () => { throw new Error("no shell"); } });
  request(target, 6);
  await idle();
}

console.log("desktop-quit unit ok");
