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
  for (const name of ["connectionStatus", "connectRepository", "connectLocalRepository", "pullConnection", "pushConnection", "resolveConnection", "retryConnection", "loginConnection", "loginConnectionComplete", "disconnectConnection"]) {
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

// The first read fills the state, and says it has arrived.
{
  const { api, calls } = routes({ connectionStatus: { connected: false, signed_in: true } });
  const web = createWebConnection({ api, project });
  assert.equal(web.state.loaded, false);
  await web.refresh();
  assert.equal(web.state.loaded, true);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
  assert.deepEqual(calls, [["connectionStatus", project]]);
}

// Connecting clones the address and shows the connected status.
{
  const { api, calls } = routes({ connectRepository: connected });
  const web = createWebConnection({ api, project });
  await web.connect("  https://github.com/you/paper.git  ");
  assert.deepEqual(calls, [["connectRepository", project, "https://github.com/you/paper.git"]]);
  assert.deepEqual(web.state.status, connected);
  assert.equal(web.state.busy, "");
}

// A connection that is refused says why, and nothing is left busy.
{
  const { api } = routes({ connectRepository: refused(409, { reason: "linked" }, "this repository is linked to another LibrePaper project") });
  const web = createWebConnection({ api, project });
  await web.connect("https://github.com/you/paper.git");
  assert.equal(web.state.problem, "this repository is linked to another LibrePaper project");
  assert.equal(web.state.busy, "");
}

// Connecting a local repository asks for the folder dialog and shows the local status.
{
  const local = { connected: true, local: true, signed_in: true, path: "/home/me/paper", branch: "main", merging: null };
  const { api, calls } = routes({ connectLocalRepository: local });
  const web = createWebConnection({ api, project });
  await web.connectLocal();
  assert.deepEqual(calls, [["connectLocalRepository", project]]);
  assert.deepEqual(web.state.status, local);
  assert.equal(web.state.problem, "");
  assert.equal(web.state.busy, "");
}

// A cancelled dialog leaves the panel as it was, with no error.
{
  const { api } = routes({ connectLocalRepository: { connected: false, signed_in: true } });
  const web = createWebConnection({ api, project });
  await web.connectLocal();
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
  assert.equal(web.state.problem, "");
  assert.equal(web.state.busy, "");
}

// A folder that is not a repository is refused with the desktop app's message.
{
  const { api } = routes({ connectLocalRepository: refused(400, { reason: "not-repository", error: "Choose the top folder of a git repository." }) });
  const web = createWebConnection({ api, project });
  await web.connectLocal();
  assert.equal(web.state.problem, "Choose the top folder of a git repository.");
  assert.equal(web.state.busy, "");
}

// A local repository on another branch refuses a transfer with the desktop app's message.
{
  const { api } = routes({ pullConnection: refused(409, { reason: "branch", error: "The repository is on branch dev, not main." }) });
  const web = createWebConnection({ api, project });
  await web.pull();
  assert.equal(web.state.problem, "The repository is on branch dev, not main.");
}

// A pull that writes files says how many came in.
{
  const { api } = routes({ pullConnection: { status: connected, pushed: { files: 3 } } });
  const web = createWebConnection({ api, project });
  await web.pull();
  assert.equal(web.state.result, "Imported from repository branch main: 3 files into the project.");
  assert.equal(web.state.refusal, null);
}

// A pull with nothing new says so.
{
  const { api } = routes({ pullConnection: { status: connected, pushed: { nothing: true } } });
  const web = createWebConnection({ api, project });
  await web.pull();
  assert.equal(web.state.result, "Imported from repository branch main: nothing new.");
}

// A pull that stops on conflicts shows the merge, and resolving each file ends it.
{
  const { api, calls } = routes({
    pullConnection: { stopped: "conflicts", status: merging },
    resolveConnection: (_project, path) => (path === "a.tex" ? mergingAfterOne : { ...connected, merging: null }),
  });
  const mergingAfterOne = { ...connected, merging: { side: "repository", conflicts: ["b.tex"] } };
  const web = createWebConnection({ api, project });
  await web.pull();
  assert.deepEqual(web.state.status.merging.conflicts, ["a.tex", "b.tex"]);
  assert.equal(web.state.result, null);
  await web.resolve("a.tex", "project");
  assert.deepEqual(web.state.status.merging.conflicts, ["b.tex"]);
  assert.equal(web.state.result, null, "the merge is not done while a file is left");
  await web.resolve("b.tex", "repository");
  assert.equal(web.state.status.merging, null);
  assert.equal(web.state.result, "The merge is done. Get repository changes or send project changes again.");
  assert.deepEqual(calls.slice(1), [["resolveConnection", project, "a.tex", "project"], ["resolveConnection", project, "b.tex", "repository"]]);
}

// A push flushes the editor's own edits first, then sends, and says what it sent.
{
  const order = [];
  const { api } = routes({ pushConnection: { status: connected, pushed: "pushed" } });
  const web = createWebConnection({
    api: { ...api, pushConnection: async (...args) => { order.push("push"); return api.pushConnection(...args); } },
    project,
    flush: async () => { order.push("flush"); },
  });
  await web.push();
  assert.deepEqual(order, ["flush", "push"]);
  assert.equal(web.state.result, "Sent to repository branch main.");
}

// A push that finds nothing new says so.
{
  const { api } = routes({ pushConnection: { status: connected, pushed: "up-to-date" } });
  const web = createWebConnection({ api, project });
  await web.push();
  assert.equal(web.state.result, "Nothing new to send to repository branch main.");
}

// A send in a local repository only takes the project in, and says so by what came in.
for (const [pulled, line] of [[true, "Sent to repository branch main."], [false, "Nothing new to send to repository branch main."]]) {
  const { api } = routes({ pushConnection: { status: connected, pulled } });
  const web = createWebConnection({ api, project });
  await web.push();
  assert.equal(web.state.result, line);
}

// A pull refused over the project's changed files keeps the paths, asks for no second
// pull, and says so.
{
  const { api, calls } = routes({ pullConnection: refused(409, { reason: "changed", paths: ["intro.tex"] }) });
  const web = createWebConnection({ api, project });
  await web.pull();
  assert.equal(calls.filter(([name]) => name === "pullConnection").length, 1);
  assert.deepEqual(web.state.refusal, { reason: "changed", paths: ["intro.tex"] });
  assert.equal(web.state.problem, "");
  assert.equal(web.state.busy, "");
}

// A suggestion stops whichever action asked, and says which.
{
  const { api } = routes({ pushConnection: refused(409, { reason: "suggestion", path: "intro.tex" }) });
  const web = createWebConnection({ api, project });
  await web.push();
  assert.deepEqual(web.state.refusal, { reason: "suggestion", path: "intro.tex", action: "push" });
}

// A push that the repository moved ahead of says to pull first.
{
  const { api } = routes({ pushConnection: refused(409, { reason: "behind" }) });
  const web = createWebConnection({ api, project });
  await web.push();
  assert.match(web.state.problem, /Get repository changes first/);
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
  const web = createWebConnection({ api, project });
  await web.refresh();
  await web.pull();
  assert.deepEqual(web.state.signInHelp, { remote: "https://github.com/you/paper.git", detail: "Authentication failed" });
  assert.equal(web.state.problem, "");
  await web.retry();
  assert.equal(web.state.signInHelp, null);
  assert.equal(web.state.result, "Imported from repository branch main: 1 file into the project.");
  web.dismiss();
}

// A 401 means the sign-in is gone, so the status says so.
{
  const { api } = routes({ connectionStatus: connected, pullConnection: refused(401, { error: "sign in" }, "sign in") });
  const web = createWebConnection({ api, project });
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
  const web = createWebConnection({ api, project, signedIn: () => true, approve: async (code) => { approved.push(code); }, wait: noWait });
  await web.login();
  assert.deepEqual(approved, ["ABCD-EFGH"]);
  assert.deepEqual(calls, [["loginConnection", project], ["loginConnectionComplete", project, "auth-1"]]);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
}

// The finish is tried again while the code is still pending.
{
  let tries = 0;
  const { api } = routes({
    loginConnection: { authorization_id: "auth-1", user_code: "ABCD-EFGH" },
    loginConnectionComplete: () => { tries += 1; if (tries < 3) throw pending(); return { connected: false, signed_in: true }; },
  });
  const web = createWebConnection({ api, project, approve: async () => {}, wait: noWait });
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
  const web = createWebConnection({ api, project, signedIn: () => false, open: (url) => opened.push(url), origin: "https://app.example", wait: noWait });
  await web.login();
  assert.deepEqual(opened, ["https://app.example/auth/device?code=ABCD-EFGH"]);
  assert.equal(polls, 2);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
}

// Disconnecting forgets the copy and answers the status.
{
  const { api, calls } = routes({ disconnectConnection: { connected: false, signed_in: true } });
  const web = createWebConnection({ api, project });
  await web.disconnect();
  assert.deepEqual(calls, [["disconnectConnection", project]]);
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
}

// A status read that started before Disconnect and lands after it is dropped, and no
// read starts while an action runs.
{
  let land = () => {};
  let reads = 0;
  const { api } = routes({
    connectionStatus: () => { reads += 1; return new Promise((resolve) => { land = () => resolve(connected); }); },
    disconnectConnection: { connected: false, signed_in: true },
  });
  const web = createWebConnection({ api, project });
  const reading = web.refresh();
  await web.disconnect();
  land();
  await reading;
  assert.deepEqual(web.state.status, { connected: false, signed_in: true });
  assert.equal(reads, 1);
}

// An answer that lands after the panel is gone is dropped.
{
  let finish;
  const { api } = routes({ connectionStatus: () => new Promise((resolve) => { finish = resolve; }) });
  const web = createWebConnection({ api, project });
  const reading = web.refresh();
  web.dispose();
  finish(connected);
  await reading;
  assert.equal(web.state.loaded, false);
  assert.equal(web.state.status, null);
}

// An interrupted send to the project stops a pull and a push. The panel keeps
// the desktop app's words and offers one retry that runs the send alone, and
// does not start the stopped transfer again.
{
  const words = "an earlier push was interrupted before its outcome was known; run `librepaper push` to settle it first";
  const stopped = refused(409, { reason: "pending", error: words });
  const scene = { pull: stopped, push: stopped };
  const { api, calls } = routes({
    pullConnection: () => scene.pull,
    pushConnection: () => scene.push,
    retryConnection: { status: connected, pushed: { files: 2 } },
  });
  const web = createWebConnection({ api, project });
  await web.pull();
  assert.equal(web.state.interrupted, words);
  assert.equal(web.state.problem, "", "it is an answer for the panel, not a failure line");
  assert.equal(web.state.result, null);

  await web.push();
  assert.equal(web.state.interrupted, words);

  await web.retryTransfer();
  assert.deepEqual(calls.filter((call) => call[0] === "retryConnection"), [["retryConnection", project]]);
  assert.equal(calls.filter((call) => call[0] === "pullConnection").length, 1, "no pull runs before or after the retry");
  assert.equal(calls.filter((call) => call[0] === "pushConnection").length, 1);
  assert.equal(web.state.interrupted, "");
  assert.match(web.state.result, /^The interrupted send finished: 2 files reached the project\. Get repository changes or send project changes again/);
  assert.equal(web.state.busy, "");

  // A transfer that goes through afterwards forgets the notice.
  scene.pull = { status: connected, pushed: { nothing: true } };
  await web.pull();
  assert.equal(web.state.interrupted, "");
}

// A retry that cannot settle keeps the button and says why.
{
  const stopped = refused(409, { reason: "pending", error: "the interrupted push was sent under a different sign-in" });
  const { api } = routes({
    pullConnection: stopped,
    retryConnection: refused(409, { reason: "pending", error: "the interrupted push was sent under a different sign-in" }),
  });
  const web = createWebConnection({ api, project });
  await web.pull();
  await web.retryTransfer();
  assert.equal(web.state.interrupted !== "", true, "the interrupted notice stays");
  assert.match(web.state.problem, /different sign-in/);

  const nothing = routes({ pullConnection: stopped, retryConnection: { status: connected, pushed: { nothing: true } } });
  const clean = createWebConnection({ api: nothing.api, project });
  await clean.pull();
  await clean.retryTransfer();
  assert.match(clean.state.result, /nothing was left to send to the project/);
}

console.log("web-connection: connect, pull, push, conflicts, refusals, interrupted sends, sign-in and disconnect");
