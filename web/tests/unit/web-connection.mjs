// The desktop app's connection of a web project to a git repository, over a
// stand-in for the connections routes: what each action asks, what it says
// back, and the refusals the panel shows instead of failing.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createWebConnection } = await loadRunes(new URL("../../src/lib/reader/web-connection.svelte.js", import.meta.url));

const refused = (status, body, message = body?.error || "The LibrePaper desktop app refused the request.") =>
  Object.assign(new Error(message), { name: "Refused", status, body });
const pending = () => refused(400, { error: "sign in and approve the code, then retry" });
const noWait = async () => {};
const project = "https://app.example/docs/paper";

// A stand-in for the routes. Each answer is a value, an Error to throw, or a
// function of the call. `calls` records what was asked, project first.
function routes(answers) {
  const calls = [];
  const api = {};
  for (const name of ["connectionStatus", "connectRepository", "pullConnection", "pushConnection", "resolveConnection", "loginConnection", "loginConnectionComplete", "disconnectConnection"]) {
    api[name] = async (...args) => {
      calls.push([name, ...args]);
      const answer = typeof answers[name] === "function" ? answers[name](...args) : answers[name];
      if (answer instanceof Error) throw answer;
      return answer;
    };
  }
  return { api, calls };
}

const connected = { connected: true, signed_in: true, url: "https://github.com/you/paper.git", branch: "main", merging: null };
const merging = { ...connected, merging: { side: "repository", conflicts: ["a.tex", "b.tex"] } };
const named = [];
const nameVersion = async (message) => { named.push(message); };

// The first read fills the state, and says it has arrived.
{
  const { api, calls } = routes({ connectionStatus: { connected: false, signed_in: true } });
  const web = createWebConnection({ api, project, nameVersion });
  assert.equal(web.state.loaded, false);
  await web.refresh();
  assert.equal(web.state.loaded, true);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
  assert.deepEqual(calls, [["connectionStatus", project]]);
}

// Connecting clones the address and shows the connected status.
{
  const { api, calls } = routes({ connectRepository: connected });
  const web = createWebConnection({ api, project, nameVersion });
  await web.connect("  https://github.com/you/paper.git  ");
  assert.deepEqual(calls, [["connectRepository", project, "https://github.com/you/paper.git"]]);
  assert.deepEqual(web.state.status, connected);
  assert.equal(web.state.busy, "");
}

// A connection that is refused says why, and nothing is left busy.
{
  const { api } = routes({ connectRepository: refused(409, { reason: "linked" }, "this repository is linked to another LibrePaper project") });
  const web = createWebConnection({ api, project, nameVersion });
  await web.connect("https://github.com/you/paper.git");
  assert.equal(web.state.problem, "this repository is linked to another LibrePaper project");
  assert.equal(web.state.busy, "");
}

// A pull that writes files says how many came in.
{
  const { api } = routes({ pullConnection: { status: connected, pushed: { files: 3 } } });
  const web = createWebConnection({ api, project, nameVersion });
  await web.pull();
  assert.equal(web.state.result, "Pulled 3 files into the project.");
  assert.equal(web.state.refusal, null);
}

// A pull with nothing new says so.
{
  const { api } = routes({ pullConnection: { status: connected, pushed: { nothing: true } } });
  const web = createWebConnection({ api, project, nameVersion });
  await web.pull();
  assert.equal(web.state.result, "Nothing new in the repository.");
}

// A pull that stops on conflicts shows the merge, and resolving each file ends it.
{
  const { api, calls } = routes({
    pullConnection: { stopped: "conflicts", status: merging },
    resolveConnection: (_project, path) => (path === "a.tex" ? mergingAfterOne : { ...connected, merging: null }),
  });
  const mergingAfterOne = { ...connected, merging: { side: "repository", conflicts: ["b.tex"] } };
  const web = createWebConnection({ api, project, nameVersion });
  await web.pull();
  assert.deepEqual(web.state.status.merging.conflicts, ["a.tex", "b.tex"]);
  assert.equal(web.state.result, null);
  await web.resolve("a.tex", "project");
  assert.deepEqual(web.state.status.merging.conflicts, ["b.tex"]);
  assert.equal(web.state.result, null, "the merge is not done while a file is left");
  await web.resolve("b.tex", "repository");
  assert.equal(web.state.status.merging, null);
  assert.equal(web.state.result, "The merge is done. Pull or Push again.");
  assert.deepEqual(calls.slice(1), [["resolveConnection", project, "a.tex", "project"], ["resolveConnection", project, "b.tex", "repository"]]);
}

// A push names the version before it sends anything, and says what it pushed.
{
  const order = [];
  const { api } = routes({ pushConnection: { status: connected, pushed: "pushed" } });
  const web = createWebConnection({
    api: { ...api, pushConnection: async (...args) => { order.push("push"); return api.pushConnection(...args); } },
    project,
    nameVersion: async (message) => { order.push(`name ${message}`); },
  });
  await web.push("Edits from LibrePaper");
  assert.deepEqual(order, ["name Edits from LibrePaper", "push"]);
  assert.equal(web.state.result, "Pushed to main.");
}

// A push that finds nothing new says so.
{
  const { api } = routes({ pushConnection: { status: connected, pushed: "up-to-date" } });
  const web = createWebConnection({ api, project, nameVersion });
  await web.push("m");
  assert.equal(web.state.result, "The repository was already up to date.");
}

// A pull that finds the project changed on the files keeps the paths and says to pull again.
{
  const { api } = routes({ pullConnection: refused(409, { reason: "changed", paths: ["intro.tex"] }) });
  const web = createWebConnection({ api, project, nameVersion });
  await web.pull();
  assert.deepEqual(web.state.refusal, { reason: "changed", paths: ["intro.tex"] });
  assert.equal(web.state.problem, "");
  assert.equal(web.state.busy, "");
}

// A suggestion stops whichever action asked, and says which.
{
  const { api } = routes({ pushConnection: refused(409, { reason: "suggestion", path: "intro.tex" }) });
  const web = createWebConnection({ api, project, nameVersion });
  await web.push("m");
  assert.deepEqual(web.state.refusal, { reason: "suggestion", path: "intro.tex", action: "push" });
}

// A push that the repository moved ahead of says to pull first.
{
  const { api } = routes({ pushConnection: refused(409, { reason: "behind" }) });
  const web = createWebConnection({ api, project, nameVersion });
  await web.push("m");
  assert.match(web.state.problem, /Pull first/);
}

// A git failure to sign in fills the sign-in help, and Try again repeats the pull.
{
  let attempts = 0;
  const { api } = routes({
    connectionStatus: connected,
    pullConnection: () => {
      attempts += 1;
      if (attempts === 1) throw refused(502, { reason: "auth", detail: "Authentication failed" }, "Authentication failed");
      return { status: connected, pushed: { files: 1 } };
    },
  });
  const web = createWebConnection({ api, project, nameVersion });
  await web.refresh();
  await web.pull();
  assert.deepEqual(web.state.signInHelp, { remote: "https://github.com/you/paper.git", detail: "Authentication failed" });
  assert.equal(web.state.problem, "");
  await web.retry();
  assert.equal(web.state.signInHelp, null);
  assert.equal(web.state.result, "Pulled 1 file into the project.");
  web.dismiss();
}

// A 401 means the sign-in is gone, so the status says so.
{
  const { api } = routes({ connectionStatus: connected, pullConnection: refused(401, { error: "sign in" }, "sign in") });
  const web = createWebConnection({ api, project, nameVersion });
  await web.refresh();
  await web.pull();
  assert.equal(web.state.status.signed_in, false);
  assert.equal(web.state.problem, "Sign in to continue.");
}

// Signed in, the page approves the code itself, then finishes the sign-in.
{
  const approved = [];
  const { api, calls } = routes({
    loginConnection: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    loginConnectionComplete: { connected: false, signed_in: true },
  });
  const web = createWebConnection({ api, project, nameVersion, signedIn: () => true, approve: async (code) => { approved.push(code); }, wait: noWait });
  await web.login();
  assert.deepEqual(approved, ["ABCD-EFGH"]);
  assert.deepEqual(calls.slice(1), [["loginConnection", project], ["loginConnectionComplete", project, "auth-1"]]);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
}

// The finish is tried again while the code is still pending.
{
  let tries = 0;
  const { api } = routes({
    loginConnection: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    loginConnectionComplete: () => { tries += 1; if (tries < 3) throw pending(); return { connected: false, signed_in: true }; },
  });
  const web = createWebConnection({ api, project, nameVersion, approve: async () => {}, wait: noWait });
  await web.login();
  assert.equal(tries, 3);
  assert.equal(web.state.problem, "");
}

// Without a session on the server the page opens the approval page and waits for the code.
{
  const opened = [];
  let polls = 0;
  const { api } = routes({
    loginConnection: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    loginConnectionComplete: () => { polls += 1; if (polls < 2) throw pending(); return { connected: false, signed_in: true }; },
  });
  const web = createWebConnection({ api, project, nameVersion, signedIn: () => false, open: (url) => opened.push(url), wait: noWait });
  await web.login();
  assert.deepEqual(opened, ["https://app.example/auth/device?code=ABCD-EFGH"]);
  assert.equal(polls, 2);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
}

// Disconnecting forgets the copy and answers the status.
{
  const { api, calls } = routes({ disconnectConnection: { connected: false, signed_in: true } });
  const web = createWebConnection({ api, project, nameVersion });
  await web.disconnect();
  assert.deepEqual(calls, [["disconnectConnection", project]]);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
}

// An answer that lands after the panel is gone is dropped.
{
  let finish;
  const { api } = routes({ connectionStatus: () => new Promise((resolve) => { finish = resolve; }) });
  const web = createWebConnection({ api, project, nameVersion });
  const reading = web.refresh();
  web.dispose();
  finish(connected);
  await reading;
  assert.equal(web.state.loaded, false);
  assert.equal(web.state.status, null);
}

console.log("web-connection: connect, pull, push, conflicts, refusals, sign-in and disconnect");
