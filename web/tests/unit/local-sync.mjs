// The Sync panel's state for a folder project, over a stand-in for the desktop
// app's sync routes: what each action asks, what it says back, and the
// refusals the panel shows instead of failing.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createLocalSync } = await loadRunes(new URL("../../src/lib/reader/local-sync.svelte.js", import.meta.url));

const refused = (status, body, message = body?.error || "The LibrePaper desktop app refused the request.") =>
  Object.assign(new Error(message), { name: "Refused", status, body });
const pending = () => refused(400, { error: "sign in and approve the code, then retry" });
const noWait = async () => {};

// A stand-in for the routes. Each answer is a value, an Error to throw, or a
// function of the call; `calls` records what was asked, project id first.
function routes(answers) {
  const calls = [];
  const api = {};
  for (const name of ["syncStatus", "syncLogin", "syncLoginComplete", "syncLink", "syncPublish", "syncUnlink", "syncPull", "syncPush"]) {
    api[name] = async (...args) => {
      calls.push([name, ...args]);
      const answer = typeof answers[name] === "function" ? answers[name](...args) : answers[name];
      if (answer instanceof Error) throw answer;
      return answer;
    };
  }
  return { api, calls };
}

const linked = { signed_in: true, linked: true, site: "https://app.example", slug: "paper", pulled: 4, changes: { here: 2, web: true } };
const today = () => new Date(2026, 9, 25, 12);

// The first read fills the state, and says it has arrived.
{
  const { api, calls } = routes({ syncStatus: linked });
  const sync = createLocalSync({ id: "f1", api, now: today });
  assert.equal(sync.state.loaded, false);
  assert.equal(sync.state.status, null);
  await sync.refresh();
  assert.equal(sync.state.loaded, true);
  assert.deepEqual(sync.state.status, linked);
  assert.deepEqual(calls, [["syncStatus", "f1"]]);
  assert.equal(sync.defaultMessage(), "2026-10-25", "a push is named by today's date until the person names it");
}

// A read that fails still says it has arrived, and says why.
{
  const { api } = routes({ syncStatus: new Error("The LibrePaper desktop app could not reach its local service.") });
  const sync = createLocalSync({ id: "f1", api });
  await sync.refresh();
  assert.equal(sync.state.loaded, true);
  assert.equal(sync.state.status, null);
  assert.match(sync.state.error, /could not reach/);
}

// An answer that arrives after a newer request was made is dropped.
{
  const waiting = [];
  const { api } = routes({ syncStatus: () => new Promise((resolve) => waiting.push(resolve)) });
  const sync = createLocalSync({ id: "f1", api });
  const first = sync.refresh();
  const second = sync.refresh();
  waiting[1]({ ...linked, pulled: 9 });
  await second;
  waiting[0]({ ...linked, pulled: 1 });
  await first;
  assert.equal(sync.state.status.pulled, 9, "the older read does not replace the newer one");
}

// Signing in asks the desktop app for a code, approves it in the page's own
// session, and finishes with the code's authorization.
{
  const order = [];
  const { api, calls } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: () => { order.push("login"); return { authorization_id: "auth-1", user_code: "ABCD-EFGH" }; },
    syncLoginComplete: () => { order.push("complete"); return { signed_in: true, linked: false }; },
  });
  const sync = createLocalSync({ id: "f1", api, wait: noWait, approve: async (code) => { order.push(`approve ${code}`); } });
  await sync.refresh();
  assert.equal(sync.state.status.signed_in, false);
  await sync.login();
  assert.deepEqual(order, ["login", "approve ABCD-EFGH", "complete"]);
  assert.deepEqual(calls.find((call) => call[0] === "syncLoginComplete"), ["syncLoginComplete", "f1", "auth-1"]);
  assert.equal(sync.state.status.signed_in, true, "the answer to the second call is the new state");
  assert.equal(sync.state.error, "");
  assert.equal(sync.state.busy, "");
}

// While the approval has not reached the desktop app the second call is tried again.
{
  let attempts = 0;
  const waits = [];
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => {
      attempts += 1;
      if (attempts < 3) return pending();
      return { signed_in: true, linked: false };
    },
  });
  const sync = createLocalSync({ id: "f1", api, wait: async (ms) => { waits.push(ms); }, approve: async () => {} });
  await sync.login();
  assert.equal(attempts, 3);
  assert.equal(waits.length, 2, "it waits between the tries");
  assert.equal(sync.state.status.signed_in, true);
}

// A code that stays pending is given up on, with the desktop app's words.
{
  let attempts = 0;
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => { attempts += 1; return pending(); },
  });
  const sync = createLocalSync({ id: "f1", api, wait: noWait, approve: async () => {} });
  await sync.refresh();
  await sync.login();
  assert.equal(attempts, 5, "a few tries, not forever");
  assert.match(sync.state.error, /approve the code/);
  assert.equal(sync.state.status.signed_in, false, "the panel is back at the Sign in button");
  assert.equal(sync.state.busy, "");
}

// Any other refusal means the sign-in expired, and is not tried again.
{
  let attempts = 0;
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => { attempts += 1; return refused(400, { error: "that sign-in expired" }); },
  });
  const sync = createLocalSync({ id: "f1", api, wait: noWait, approve: async () => {} });
  await sync.refresh();
  await sync.login();
  assert.equal(attempts, 1);
  assert.equal(sync.state.error, "that sign-in expired");
  assert.equal(sync.state.status.signed_in, false);
}

// A code the page could not approve never reaches the second call.
{
  const { api, calls } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: { signed_in: true },
  });
  const sync = createLocalSync({ id: "f1", api, wait: noWait, approve: async () => { throw new Error("Sign in to this server first."); } });
  await sync.login();
  assert.equal(calls.some((call) => call[0] === "syncLoginComplete"), false);
  assert.equal(sync.state.error, "Sign in to this server first.");
}

// Link and publish send what was typed, trimmed, and read the state again.
{
  const scene = { status: { signed_in: true, linked: false } };
  const { api, calls } = routes({
    syncStatus: () => scene.status,
    syncLink: () => { scene.status = linked; },
    syncPublish: () => { scene.status = linked; },
    syncUnlink: () => { scene.status = { signed_in: true, linked: false }; },
  });
  const sync = createLocalSync({ id: "f1", api });
  await sync.refresh();
  await sync.link("  https://app.example/docs/paper  ");
  assert.deepEqual(calls.at(-2), ["syncLink", "f1", "https://app.example/docs/paper"]);
  assert.equal(sync.state.status.linked, true);
  await sync.unlink();
  assert.equal(calls.at(-2)[0], "syncUnlink");
  assert.equal(sync.state.status.linked, false);
  await sync.publish(" My paper ");
  assert.deepEqual(calls.at(-2), ["syncPublish", "f1", "My paper"]);
  assert.equal(sync.state.status.linked, true);
}

// Pull saves the folder first, shows the counts, and keeps the conflicts for an answer.
{
  const order = [];
  const answers = [
    { written: ["x.tex", "y.tex", "z.tex"], trashed: ["old.tex"], kept: [], conflicts: ["a.tex", "b.tex"], trimmed: false },
    { written: ["a.tex"], trashed: [], kept: [], conflicts: ["b.tex"] },
    { written: ["b.tex"], trashed: [], kept: [], conflicts: [] },
  ];
  const { api, calls } = routes({ syncStatus: linked, syncPull: () => { order.push("pull"); return answers.shift(); } });
  const sync = createLocalSync({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await sync.refresh();
  await sync.pull();
  assert.deepEqual(order, ["flush", "pull"], "the folder is saved before the pull reads it");
  assert.deepEqual(calls.find((call) => call[0] === "syncPull"), ["syncPull", "f1", {}]);
  assert.deepEqual(sync.state.pull, { written: 3, trashed: 1, skipped: [], trimmed: false });
  assert.deepEqual(sync.state.conflicts, ["a.tex", "b.tex"]);

  await sync.keepMine("a.tex");
  assert.deepEqual(calls.filter((call) => call[0] === "syncPull")[1], ["syncPull", "f1", { mine: ["a.tex"] }]);
  assert.deepEqual(sync.state.conflicts, ["b.tex"]);
  assert.equal(sync.state.pull.written, 4, "the answer adds to the pull it belongs to");

  await sync.takeTheirs("b.tex");
  assert.deepEqual(calls.filter((call) => call[0] === "syncPull")[2], ["syncPull", "f1", { theirs: ["b.tex"] }]);
  assert.deepEqual(sync.state.conflicts, []);
  assert.equal(sync.state.pull.written, 5);
}

// A new pull starts its counts over, and says when the history was trimmed.
{
  const { api } = routes({ syncStatus: linked, syncPull: { written: ["a.tex", "b.tex"], trashed: [], conflicts: [], trimmed: true } });
  const sync = createLocalSync({ id: "f1", api });
  await sync.pull();
  await sync.pull();
  assert.deepEqual(sync.state.pull, { written: 2, trashed: 0, skipped: [], trimmed: true });
}

// A folder that could not be saved stops both commands before they run.
{
  const { api, calls } = routes({ syncStatus: linked, syncPull: {}, syncPush: {} });
  const sync = createLocalSync({ id: "f1", api, flush: async () => false });
  await sync.pull();
  await sync.push("msg");
  assert.equal(calls.some((call) => call[0] === "syncPull" || call[0] === "syncPush"), false);
  assert.match(sync.state.error, /could not be saved/);
  assert.equal(sync.state.busy, "");
}

// Publish settles the folder first, and does not run when the folder could not be saved.
{
  const order = [];
  const scene = { saved: false };
  const { api, calls } = routes({ syncStatus: linked, syncPublish: () => { order.push("publish"); } });
  const sync = createLocalSync({ id: "f1", api, flush: async () => { order.push("flush"); return scene.saved; } });
  await sync.publish("My paper");
  assert.equal(calls.some((call) => call[0] === "syncPublish"), false, "a failed save stops the publish");
  assert.match(sync.state.error, /could not be saved/);
  assert.equal(sync.state.busy, "");

  scene.saved = true;
  await sync.publish("My paper");
  assert.deepEqual(order, ["flush", "flush", "publish"], "the folder is saved before the publish reads it");
  assert.deepEqual(calls.find((call) => call[0] === "syncPublish"), ["syncPublish", "f1", "My paper"]);
  assert.equal(sync.state.error, "");
}

// Push sends the message, or today's date, and says what it did.
{
  const queue = [{ label: { sha: "abc", sequence: 5 }, files: ["a.tex", "b.tex", "c.png"] }, { nothing: true }];
  const { api, calls } = routes({ syncStatus: linked, syncPush: () => queue.shift() });
  const sync = createLocalSync({ id: "f1", api, now: today });
  await sync.push("  Fix the abstract  ");
  assert.deepEqual(calls.find((call) => call[0] === "syncPush"), ["syncPush", "f1", "Fix the abstract"]);
  assert.deepEqual(sync.state.pushed, { files: 3 });
  await sync.push("   ");
  assert.deepEqual(calls.filter((call) => call[0] === "syncPush")[1], ["syncPush", "f1", "2026-10-25"], "an empty message falls back to today's date");
  assert.deepEqual(sync.state.pushed, { nothing: true });
  assert.equal(sync.state.refusal, null);
}

// A push the web project refuses is shown, not failed.
{
  const scene = { refusal: refused(409, { reason: "changed", paths: ["intro.tex", "fig/a.png"] }) };
  const { api } = routes({ syncStatus: linked, syncPush: () => scene.refusal });
  const sync = createLocalSync({ id: "f1", api });
  await sync.push("m");
  assert.deepEqual(sync.state.refusal, { reason: "changed", paths: ["intro.tex", "fig/a.png"] });
  assert.equal(sync.state.error, "", "a refusal is not an error line");
  assert.equal(sync.state.pushed, null);

  scene.refusal = refused(409, { reason: "suggestion", path: "intro.tex" });
  await sync.push("m");
  assert.deepEqual(sync.state.refusal, { reason: "suggestion", path: "intro.tex" });

  scene.refusal = new Error("The LibrePaper desktop app could not reach its local service.");
  await sync.push("m");
  assert.equal(sync.state.refusal, null, "a new push forgets the last refusal");
  assert.match(sync.state.error, /could not reach/);
}

// A sign-in that has ended sends the panel back to the Sign in button.
{
  const { api } = routes({ syncStatus: linked, syncPull: refused(401, { error: "sign in" }, "sign in") });
  const sync = createLocalSync({ id: "f1", api });
  await sync.refresh();
  assert.equal(sync.state.status.signed_in, true);
  await sync.pull();
  assert.equal(sync.state.status.signed_in, false);
  assert.match(sync.state.error, /Sign in/);
}

// One action at a time: a second press while one runs is ignored.
{
  let release;
  const { api, calls } = routes({
    syncStatus: linked,
    syncPull: () => new Promise((resolve) => { release = () => resolve({ written: [], trashed: [], conflicts: [] }); }),
  });
  const sync = createLocalSync({ id: "f1", api });
  const first = sync.pull();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sync.state.busy, "pull");
  await sync.pull();
  await sync.push("m");
  assert.equal(calls.filter((call) => call[0] === "syncPull").length, 1);
  assert.equal(calls.some((call) => call[0] === "syncPush"), false);
  release();
  await first;
  assert.equal(sync.state.busy, "");
}

// Once the panel is gone, a late answer changes nothing.
{
  let release;
  const { api } = routes({ syncStatus: () => new Promise((resolve) => { release = resolve; }) });
  const sync = createLocalSync({ id: "f1", api });
  const reading = sync.refresh();
  sync.dispose();
  release(linked);
  await reading;
  assert.equal(sync.state.loaded, false);
}

console.log("local-sync: status, link, pull with conflicts, push with refusals, sign-in loss");
