// The LibrePaper section of the Git panel for a folder project, over a stand-in for the desktop
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

// With a session on the server the code is approved here, and no tab opens.
{
  const opened = [];
  const asked = [];
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH", verification_url: "https://app.example/auth/device?code=ABCD-EFGH" },
    syncLoginComplete: { signed_in: true, linked: false },
  });
  const approved = [];
  const sync = createLocalSync({
    id: "f1", api, wait: noWait,
    signedIn: async () => { asked.push("asked"); return true; },
    open: (url) => opened.push(url),
    approve: async (code) => { approved.push(code); },
  });
  await sync.login();
  assert.deepEqual(approved, ["ABCD-EFGH"]);
  assert.deepEqual(opened, [], "no tab opens when this page can approve");
  assert.equal(sync.state.waiting, null);
  assert.equal(sync.state.status.signed_in, true);
  assert.equal(asked.length, 1, "the session is asked about each time Sign in is pressed");
}

// Without a session on the server the code is shown, the server's page opens in
// a new tab, and the finish is asked again until the code is approved there.
{
  const opened = [];
  const order = [];
  const seen = [];
  let attempts = 0;
  const waits = [];
  const sync = createLocalSync({
    id: "f1", wait: async (ms) => { waits.push(ms); }, open: (url) => opened.push(url),
    signedIn: async () => false,
    approve: async () => { order.push("approve"); },
    api: routes({
      syncStatus: { signed_in: false, linked: false },
      syncLogin: () => { order.push("login"); return { authorization_id: "auth-1", user_code: "ABCD-EFGH", verification_url: "https://app.example/auth/device?code=ABCD-EFGH" }; },
      syncLoginComplete: () => {
        attempts += 1;
        seen.push(sync.state.waiting);
        if (attempts < 4) return pending();
        return { signed_in: true, linked: false, site: "https://app.example" };
      },
    }).api,
  });
  await sync.refresh();
  await sync.login();
  assert.deepEqual(order, ["login"], "the page never approves a code it has no session for");
  assert.deepEqual(opened, ["https://app.example/auth/device?code=ABCD-EFGH"], "the desktop app's verification page opens once");
  assert.equal(attempts, 4);
  assert.deepEqual(waits, [3000, 3000, 3000, 3000], "it waits a few seconds before each ask");
  assert.ok(seen.every((waiting) => waiting?.userCode === "ABCD-EFGH" && waiting.url === opened[0]), "the code and the page are in the state while it waits");
  assert.equal(sync.state.waiting, null, "and gone once it is approved");
  assert.equal(sync.state.status.signed_in, true, "the answer to the finish is the new state");
  assert.equal(sync.state.error, "");
  assert.equal(sync.state.busy, "");
}

// A desktop app that names no verification page gets the server's device page under the page's origin.
{
  const opened = [];
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: { signed_in: true, linked: false },
  });
  const sync = createLocalSync({ id: "f1", api, wait: noWait, signedIn: () => false, origin: "https://app.example/", open: (url) => opened.push(url), approve: async () => {} });
  await sync.login();
  assert.deepEqual(opened, ["https://app.example/auth/device?code=ABCD-EFGH"]);
}

// A code that is never approved is given up on after ten minutes, with a sentence that says so.
{
  let clock = Date.UTC(2026, 9, 25, 12);
  let attempts = 0;
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => { attempts += 1; return pending(); },
  });
  const sync = createLocalSync({
    id: "f1", api, signedIn: () => false, open: () => {}, approve: async () => {},
    now: () => new Date(clock), wait: async (ms) => { clock += ms; },
  });
  await sync.refresh();
  await sync.login();
  assert.equal(attempts, 200, "an ask every three seconds for ten minutes");
  assert.match(sync.state.error, /not approved in time/);
  assert.equal(sync.state.waiting, null);
  assert.equal(sync.state.status.signed_in, false, "the panel is back at the Sign in button");
  assert.equal(sync.state.busy, "");
}

// Any other refusal while waiting means the sign-in expired, and ends the wait.
{
  let attempts = 0;
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => { attempts += 1; return attempts < 3 ? pending() : refused(400, { error: "that sign-in expired" }); },
  });
  const sync = createLocalSync({ id: "f1", api, wait: noWait, signedIn: () => false, open: () => {}, approve: async () => {} });
  await sync.refresh();
  await sync.login();
  assert.equal(attempts, 3);
  assert.equal(sync.state.error, "that sign-in expired");
  assert.equal(sync.state.waiting, null);
  assert.equal(sync.state.status.signed_in, false);
}

// Cancel ends the wait at once and the desktop app is not asked again.
{
  const idle = () => new Promise((resolve) => setImmediate(resolve));
  const gates = [];
  let attempts = 0;
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => { attempts += 1; return pending(); },
  });
  const sync = createLocalSync({ id: "f1", api, signedIn: () => false, open: () => {}, approve: async () => {}, wait: () => new Promise((resolve) => gates.push(resolve)) });
  await sync.refresh();
  const running = sync.login();
  await idle();
  assert.equal(sync.state.waiting?.userCode, "ABCD-EFGH");
  assert.equal(sync.state.busy, "login");
  gates.shift()();
  await idle();
  assert.equal(attempts, 1, "the first ask found the code pending");
  sync.cancelLogin();
  await running;
  assert.equal(sync.state.waiting, null, "the waiting state goes");
  assert.equal(sync.state.busy, "");
  assert.equal(sync.state.error, "", "a cancel is not an error");
  assert.equal(sync.state.status.signed_in, false, "the panel is back at the Sign in button");
  while (gates.length) gates.shift()();
  await idle();
  assert.equal(attempts, 1, "no ask is made after the cancel");
  sync.cancelLogin();

  const again = sync.login();
  await idle();
  assert.equal(sync.state.waiting?.userCode, "ABCD-EFGH", "Sign in can be pressed again");
  sync.cancelLogin();
  await again;
}

// A panel that goes away ends the wait too.
{
  const idle = () => new Promise((resolve) => setImmediate(resolve));
  let attempts = 0;
  const { api } = routes({
    syncStatus: { signed_in: false, linked: false },
    syncLogin: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    syncLoginComplete: () => { attempts += 1; return pending(); },
  });
  const sync = createLocalSync({ id: "f1", api, signedIn: () => false, open: () => {}, approve: async () => {}, wait: () => new Promise(() => {}) });
  const running = sync.login();
  await idle();
  sync.dispose();
  await running;
  assert.equal(attempts, 0);
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

// In a git repository the desktop app answers a pull in its repository form:
// the versions it took in as commits, then how git merged them. The controller
// keeps the counts and the word, and the paths of a merge that did not finish.
{
  const version = (n) => ({ sha: `sha${n}`, label: `v${n}`, by: "anne", commit: `c${n}` });
  const answers = [
    { repository: true, versions: [version(1), version(2), version(3)], merge: "fast-forward", trimmed: false },
    { repository: true, versions: [version(4)], merge: "merged", trimmed: true },
    { repository: true, versions: [], merge: "already", trimmed: false },
    { repository: true, versions: [version(5)], merge: "skipped", trimmed: false },
    { repository: true, versions: [version(6)], merge: { conflicts: ["a.tex", "b.tex"] }, trimmed: false },
    { repository: true, versions: [], merge: { dirty: ["c.tex"] }, trimmed: false },
  ];
  const { api, calls } = routes({ syncStatus: { ...linked, git: true }, syncPull: () => answers.shift() });
  const sync = createLocalSync({ id: "f1", api });
  await sync.refresh();
  assert.equal(sync.state.status.git, true, "a git folder keeps the whole LibrePaper section; the flag only picks the wording");
  await sync.pull();
  assert.deepEqual(sync.state.pull, { repository: true, versions: 3, merge: "fast-forward", paths: [], trimmed: false });
  assert.deepEqual(sync.state.conflicts, [], "there is no file to choose a side for");
  await sync.pull();
  assert.deepEqual(sync.state.pull, { repository: true, versions: 1, merge: "merged", paths: [], trimmed: true });
  await sync.pull();
  assert.deepEqual(sync.state.pull, { repository: true, versions: 0, merge: "already", paths: [], trimmed: false });
  await sync.pull();
  assert.equal(sync.state.pull.merge, "skipped");
  await sync.pull();
  assert.deepEqual(sync.state.pull, { repository: true, versions: 1, merge: "conflicts", paths: ["a.tex", "b.tex"], trimmed: false });
  assert.deepEqual(sync.state.conflicts, [], "git's conflicts are resolved in the editor, not by Keep mine and Take theirs");
  await sync.pull();
  assert.deepEqual(sync.state.pull, { repository: true, versions: 0, merge: "dirty", paths: ["c.tex"], trimmed: false });
  assert.equal(calls.filter((call) => call[0] === "syncPull").length, 6);
  assert.equal(sync.state.error, "");

  // Push on a git folder answers the same shapes as before.
  const pushes = routes({ syncStatus: { ...linked, git: true }, syncPush: { label: { sha: "abc", sequence: 5 }, files: 2 } });
  const pushing = createLocalSync({ id: "f1", api: pushes.api, now: today });
  await pushing.push("Fix");
  assert.deepEqual(pushing.state.pushed, { files: 2 });
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

console.log("local-sync: status, sign-in by approving here or in another tab, link, pull with conflicts, push with refusals, sign-in loss");
