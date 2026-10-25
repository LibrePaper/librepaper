// The Repository section's state for a folder project, over a stand-in for the
// desktop app's git routes: what each action asks, what it says back, the
// folder save that comes before the actions that read the disk, and the
// refusals the panel shows instead of failing.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createLocalGit, SYNC_MESSAGE } = await loadRunes(new URL("../../src/lib/reader/local-git.svelte.js", import.meta.url));

const refused = (status, body, message = body?.error || "The LibrePaper desktop app refused the request.") =>
  Object.assign(new Error(message), { name: "Refused", status, body });
const idle = () => new Promise((resolve) => setImmediate(resolve));

// A stand-in for the routes. Each answer is a value, an Error to throw, or a
// function of the call; `calls` records what was asked, project id first.
function routes(answers) {
  const calls = [];
  const api = {};
  for (const name of ["gitStatus", "gitInit", "gitDiff", "gitCommit", "gitIdentity", "gitBranches", "gitSwitch", "gitBranch", "gitRemote", "gitFetch", "gitPull", "gitPush", "gitSync"]) {
    api[name] = async (...args) => {
      calls.push([name, ...args]);
      const answer = typeof answers[name] === "function" ? answers[name](...args) : answers[name];
      if (answer instanceof Error) throw answer;
      return answer;
    };
  }
  return { api, calls };
}
const asked = (calls, name) => calls.filter((call) => call[0] === name);

const repo = {
  repository: true, git: { found: true, version: "2.43.0" }, branch: "main", upstream: "origin/main", ahead: 0, behind: 0,
  remotes: [{ name: "origin", url: "https://github.com/you/paper.git" }],
  changes: [{ path: "intro.tex", kind: "modified" }, { path: "fig/a.png", kind: "added" }],
  merging: false, identity: true,
};
const noFlush = async () => true;

// The first read fills the state, and says it has arrived.
{
  const { api, calls } = routes({ gitStatus: repo });
  const git = createLocalGit({ id: "f1", api });
  assert.equal(git.state.loaded, false);
  assert.equal(git.state.status, null);
  await git.refresh();
  assert.equal(git.state.loaded, true);
  assert.deepEqual(git.state.status, repo);
  assert.deepEqual(calls, [["gitStatus", "f1"]]);
  assert.equal(git.defaultMessage(), "", "an ordinary commit has no message until one is written");
}

// A merge in progress offers git's own words for the commit that finishes it.
{
  const { api } = routes({ gitStatus: { ...repo, merging: true } });
  const git = createLocalGit({ id: "f1", api });
  await git.refresh();
  assert.equal(git.defaultMessage(), "Merge origin/main");
}

// A read that fails still says it has arrived, and says why.
{
  const { api } = routes({ gitStatus: new Error("The LibrePaper desktop app could not reach its local service.") });
  const git = createLocalGit({ id: "f1", api });
  await git.refresh();
  assert.equal(git.state.loaded, true);
  assert.equal(git.state.status, null);
  assert.match(git.state.error, /could not reach/);
}

// A good read clears the message of a failed read, and leaves what an action said.
{
  const scene = { answer: new Error("The LibrePaper desktop app could not reach its local service.") };
  const { api } = routes({ gitStatus: () => scene.answer, gitPush: refused(500, { error: "git push failed" }) });
  const git = createLocalGit({ id: "f1", api });
  await git.refresh();
  assert.match(git.state.error, /could not reach/);
  scene.answer = repo;
  await git.refresh();
  assert.equal(git.state.error, "", "the read worked, so its complaint goes");
  await git.push();
  assert.equal(git.state.error, "git push failed");
  await git.refresh();
  assert.equal(git.state.error, "git push failed", "a later read does not take back what a push said");
}

// An answer that arrives after a newer request was made is dropped.
{
  const waiting = [];
  const { api } = routes({ gitStatus: () => new Promise((resolve) => waiting.push(resolve)) });
  const git = createLocalGit({ id: "f1", api });
  const first = git.refresh();
  const second = git.refresh();
  waiting[1]({ ...repo, branch: "new" });
  await second;
  waiting[0]({ ...repo, branch: "old" });
  await first;
  assert.equal(git.state.status.branch, "new", "the older read does not replace the newer one");
}

// A folder with no repository is initialized, and the state is read again.
{
  const scene = { status: { repository: false, git: { found: true, version: "2.43.0" }, remotes: [], changes: [], merging: false, identity: false } };
  const { api, calls } = routes({ gitStatus: () => scene.status, gitInit: () => { scene.status = repo; } });
  const git = createLocalGit({ id: "f1", api });
  await git.refresh();
  assert.equal(git.state.status.repository, false);
  await git.init();
  assert.deepEqual(asked(calls, "gitInit"), [["gitInit", "f1"]]);
  assert.equal(git.state.status.repository, true);
  assert.match(git.state.steps[0].text, /git repository on the branch main/);
  assert.equal(git.state.busy, "");
}

// Commit saves the folder first, sends the message trimmed, and says what it made.
{
  const order = [];
  const { api, calls } = routes({
    gitStatus: repo,
    gitCommit: () => { order.push("commit"); return { commit: "1a2b3c4d5e6f7a8b" }; },
  });
  const git = createLocalGit({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await git.commit("  Fix the abstract  ");
  assert.deepEqual(order, ["flush", "commit"], "the folder is saved before git reads it");
  assert.deepEqual(asked(calls, "gitCommit"), [["gitCommit", "f1", "Fix the abstract", undefined]], "everything is committed by default");
  assert.deepEqual(git.state.steps, [{ step: "commit", text: "Committed 1a2b3c4." }]);
  assert.equal(asked(calls, "gitStatus").length, 1, "and the state is read again");

  await git.commit("Only this file", ["intro.tex"]);
  assert.deepEqual(asked(calls, "gitCommit")[1], ["gitCommit", "f1", "Only this file", ["intro.tex"]], "a file can be left out");
}

// A commit without a message does not ask, and neither does one the folder could not be saved for.
{
  const { api, calls } = routes({ gitStatus: repo, gitCommit: {} });
  const git = createLocalGit({ id: "f1", api });
  await git.commit("   ");
  assert.equal(asked(calls, "gitCommit").length, 0);
  assert.match(git.state.error, /message/);

  const stopped = createLocalGit({ id: "f1", api, flush: async () => false });
  await stopped.commit("Fix");
  assert.equal(asked(calls, "gitCommit").length, 0);
  assert.match(stopped.state.error, /could not be saved/);
  assert.equal(stopped.state.busy, "");
}

// A missing identity opens the prompt instead of failing, and saving it commits again.
{
  let identified = false;
  const { api, calls } = routes({
    gitStatus: repo,
    gitCommit: () => (identified ? { commit: "abcdef0123" } : refused(409, { reason: "identity" })),
    gitIdentity: () => { identified = true; },
  });
  const git = createLocalGit({ id: "f1", api });
  await git.commit("Fix the abstract", ["intro.tex"]);
  assert.deepEqual(git.state.identity, { then: "commit", message: "Fix the abstract", paths: ["intro.tex"] });
  assert.equal(git.state.error, "", "a missing identity is a question, not an error");
  assert.deepEqual(git.state.steps, []);

  await git.saveIdentity("  ", "anne@example.org");
  assert.match(git.state.error, /name and an email/, "both are needed");
  assert.equal(asked(calls, "gitIdentity").length, 0);
  await git.saveIdentity("Anne Example", "not an address");
  assert.equal(asked(calls, "gitIdentity").length, 0);

  await git.saveIdentity(" Anne Example ", " anne@example.org ");
  assert.deepEqual(asked(calls, "gitIdentity"), [["gitIdentity", "f1", "Anne Example", "anne@example.org"]]);
  assert.equal(git.state.identity, null);
  assert.equal(git.state.error, "");
  assert.deepEqual(asked(calls, "gitCommit")[1], ["gitCommit", "f1", "Fix the abstract", ["intro.tex"]], "the commit that was waiting is made, with the same message and files");
  assert.equal(git.state.steps[0].text, "Committed abcdef0.");
  assert.equal(git.state.busy, "");
}

// The prompt can be put away, and an identity the desktop app refuses is said.
{
  const { api } = routes({
    gitStatus: repo,
    gitCommit: refused(409, { reason: "identity" }),
    gitIdentity: refused(400, { error: "That email address is not valid." }),
  });
  const git = createLocalGit({ id: "f1", api });
  await git.commit("Fix");
  assert.ok(git.state.identity);
  await git.saveIdentity("Anne", "anne@example.org");
  assert.equal(git.state.error, "That email address is not valid.");
  assert.ok(git.state.identity, "the prompt stays so it can be fixed");
  assert.equal(git.state.busy, "");
  git.cancelIdentity();
  assert.equal(git.state.identity, null);
  assert.equal(git.state.error, "");
}

// Any other commit failure is said as the desktop app said it.
{
  const { api } = routes({ gitStatus: repo, gitCommit: refused(500, { error: "git commit failed: hook rejected" }) });
  const git = createLocalGit({ id: "f1", api });
  await git.commit("Fix");
  assert.equal(git.state.error, "git commit failed: hook rejected");
  assert.equal(git.state.identity, null);
}

// The branches are listed for the menu.
{
  const { api, calls } = routes({ gitBranches: { current: "main", branches: ["main", "draft"] } });
  const git = createLocalGit({ id: "f1", api });
  assert.equal(git.state.branches, null);
  await git.loadBranches();
  assert.deepEqual(git.state.branches, { current: "main", names: ["main", "draft"] });
  assert.deepEqual(calls, [["gitBranches", "f1"]]);
}

// Switching saves the folder first, then reads the state and the branches again.
{
  const order = [];
  const { api, calls } = routes({
    gitStatus: repo,
    gitBranches: { current: "draft", branches: ["main", "draft"] },
    gitSwitch: () => { order.push("switch"); },
  });
  const git = createLocalGit({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await git.switchBranch("draft");
  assert.deepEqual(order, ["flush", "switch"], "uncommitted edits are on disk before git moves the files");
  assert.deepEqual(asked(calls, "gitSwitch"), [["gitSwitch", "f1", "draft"]]);
  assert.equal(git.state.steps[0].text, "Switched to draft.");
  assert.equal(asked(calls, "gitStatus").length, 1);
  assert.equal(git.state.branches.current, "draft");
}

// A switch that would overwrite uncommitted work lists the files, and any other refusal is said.
{
  const scene = { answer: refused(409, { reason: "dirty", paths: ["intro.tex"] }) };
  const { api } = routes({ gitStatus: repo, gitSwitch: () => scene.answer });
  const git = createLocalGit({ id: "f1", api });
  await git.switchBranch("draft");
  assert.deepEqual(git.state.refusal, { reason: "dirty", paths: ["intro.tex"] });
  assert.equal(git.state.error, "");
  scene.answer = refused(400, { error: "pathspec 'nope' did not match" });
  await git.switchBranch("nope");
  assert.equal(git.state.error, "pathspec 'nope' did not match");
  assert.equal(git.state.refusal, null, "the earlier refusal is gone");
}

// A new branch needs a name, and the folder is saved first.
{
  const order = [];
  const { api, calls } = routes({
    gitStatus: repo,
    gitBranches: { current: "review", branches: ["main", "review"] },
    gitBranch: () => { order.push("branch"); },
  });
  const git = createLocalGit({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await git.createBranch("   ");
  assert.equal(asked(calls, "gitBranch").length, 0);
  assert.match(git.state.error, /name/);
  await git.createBranch(" review ");
  assert.deepEqual(order, ["flush", "branch"]);
  assert.deepEqual(asked(calls, "gitBranch"), [["gitBranch", "f1", "review"]]);
  assert.match(git.state.steps[0].text, /review/);
  assert.equal(git.state.error, "");
}

// A remote is checked before it is sent: no password, only the three forms.
{
  const { api, calls } = routes({ gitStatus: repo, gitRemote: {} });
  const git = createLocalGit({ id: "f1", api });
  await git.setRemote("https://you:secret@github.com/you/paper.git");
  assert.equal(asked(calls, "gitRemote").length, 0, "a password never leaves the page");
  assert.match(git.state.error, /password/);
  await git.setRemote("/home/you/bare.git");
  assert.equal(asked(calls, "gitRemote").length, 0);
  assert.match(git.state.error, /https:\/\/ or ssh:\/\//);

  await git.setRemote("  git@github.com:you/paper.git  ");
  assert.deepEqual(asked(calls, "gitRemote"), [["gitRemote", "f1", "origin", "git@github.com:you/paper.git"]], "the remote is origin unless it is named");
  assert.equal(git.state.error, "");
  assert.equal(asked(calls, "gitStatus").length, 1);
  await git.setRemote("https://example.org/paper.git", "upstream");
  assert.deepEqual(asked(calls, "gitRemote")[1], ["gitRemote", "f1", "upstream", "https://example.org/paper.git"]);
}

// A fetch says how far the branch is from its upstream.
{
  const { api, calls } = routes({ gitStatus: repo, gitFetch: { ahead: 2, behind: 1 } });
  const git = createLocalGit({ id: "f1", api });
  await git.fetch();
  assert.deepEqual(asked(calls, "gitFetch"), [["gitFetch", "f1"]]);
  assert.deepEqual(git.state.steps, [{ step: "fetch", text: "Fetched. 2 ahead, 1 behind." }]);
  assert.equal(asked(calls, "gitStatus").length, 1);
}

// Pull saves the folder first and says how it went.
{
  const order = [];
  const answers = [
    { status: "fast-forward", conflicts: [] },
    { status: "merged", conflicts: [] },
    { status: "up-to-date", conflicts: [] },
  ];
  const { api, calls } = routes({ gitStatus: repo, gitPull: () => { order.push("pull"); return answers.shift(); } });
  const git = createLocalGit({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await git.pull();
  assert.deepEqual(order, ["flush", "pull"], "the folder is saved before the pull reads it");
  assert.equal(git.state.steps[0].text, "Pulled and fast-forwarded.");
  await git.pull();
  assert.equal(git.state.steps[0].text, "Pulled and merged.");
  await git.pull();
  assert.equal(git.state.steps[0].text, "Already up to date.");
  assert.equal(git.state.conflicts.length, 0);
  assert.equal(asked(calls, "gitPull").length, 3);
}

// A pull that conflicts lists the files; the merge stays in progress for the editor.
{
  const { api } = routes({ gitStatus: { ...repo, merging: true }, gitPull: { status: "conflicts", conflicts: ["intro.tex", "refs.bib"] } });
  const git = createLocalGit({ id: "f1", api });
  await git.pull();
  assert.deepEqual(git.state.conflicts, ["intro.tex", "refs.bib"]);
  assert.equal(git.state.steps[0].failed, true);
  assert.match(git.state.steps[0].text, /2 files in conflict/);
  assert.equal(git.state.error, "", "a conflict is an answer, not an error");
  assert.equal(git.state.status.merging, true, "the state is read again, and a merge is in progress");
}

// A pull over uncommitted files lists them and does not start.
{
  const { api } = routes({ gitStatus: repo, gitPull: refused(409, { reason: "dirty", paths: ["intro.tex", "refs.bib"] }) });
  const git = createLocalGit({ id: "f1", api });
  await git.pull();
  assert.deepEqual(git.state.refusal, { reason: "dirty", paths: ["intro.tex", "refs.bib"] });
  assert.equal(git.state.error, "");
  assert.deepEqual(git.state.steps, []);
}

// A pull is stopped by a folder that could not be saved.
{
  const { api, calls } = routes({ gitStatus: repo, gitPull: {} });
  const git = createLocalGit({ id: "f1", api, flush: async () => false });
  await git.pull();
  assert.equal(asked(calls, "gitPull").length, 0);
  assert.match(git.state.error, /could not be saved/);
}

// A push says it pushed. It does not save the folder, because it sends commits.
{
  const order = [];
  const { api, calls } = routes({ gitStatus: repo, gitPush: () => { order.push("push"); } });
  const git = createLocalGit({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await git.push();
  assert.deepEqual(order, ["push"]);
  assert.deepEqual(asked(calls, "gitPush"), [["gitPush", "f1"]]);
  assert.deepEqual(git.state.steps, [{ step: "push", text: "Pushed." }]);
}

// A push to a remote that moved says to pull first.
{
  const { api } = routes({ gitStatus: repo, gitPush: refused(409, { reason: "behind" }) });
  const git = createLocalGit({ id: "f1", api });
  await git.push();
  assert.deepEqual(git.state.refusal, { reason: "behind" });
  assert.equal(git.state.error, "");
  assert.deepEqual(git.state.steps, []);
}

// A connection that failed carries git's words; a sign-in that failed carries what the help needs.
{
  const scene = { answer: refused(409, { reason: "network", detail: "Could not resolve host: github.com" }) };
  const { api, calls } = routes({ gitStatus: repo, gitPush: () => scene.answer, gitPull: () => scene.answer, gitFetch: () => scene.answer });
  const git = createLocalGit({ id: "f1", api });
  await git.push();
  assert.deepEqual(git.state.refusal, { reason: "network", detail: "Could not resolve host: github.com" });
  assert.equal(git.state.auth, null);

  scene.answer = refused(409, { reason: "auth", detail: "Permission denied (publickey)." });
  await git.push();
  assert.deepEqual(git.state.auth, { detail: "Permission denied (publickey).", action: "push" });
  assert.equal(git.state.refusal, null, "the connection failure is gone");
  assert.equal(git.state.error, "", "a sign-in problem has its own help, not an error line");

  // Try again repeats the action that failed, once the person has fixed the sign-in.
  scene.answer = undefined;
  await git.retrySignIn();
  assert.equal(asked(calls, "gitPush").length, 3);
  assert.equal(git.state.auth, null);
  assert.equal(git.state.steps[0].text, "Pushed.");

  // The same goes for a pull and a fetch.
  scene.answer = refused(409, { reason: "auth", detail: "x" });
  await git.pull();
  assert.equal(git.state.auth.action, "pull");
  await git.fetch();
  assert.equal(git.state.auth.action, "fetch");
  scene.answer = { status: "up-to-date", conflicts: [], ahead: 0, behind: 0 };
  await git.retrySignIn();
  assert.equal(asked(calls, "gitFetch").length, 2);
  git.dismiss();
  assert.equal(git.state.auth, null);
  await git.retrySignIn();
  assert.equal(asked(calls, "gitFetch").length, 2, "with nothing to retry it does nothing");
}

// Sync saves the folder, commits with the default words, and lists every step.
{
  const order = [];
  const steps = [{ step: "commit", commit: "1a2b3c4d" }, { step: "pull", status: "merged", conflicts: [] }, { step: "push" }];
  const { api, calls } = routes({ gitStatus: repo, gitSync: () => { order.push("sync"); return { steps }; } });
  const git = createLocalGit({ id: "f1", api, flush: async () => { order.push("flush"); return true; } });
  await git.sync();
  assert.deepEqual(order, ["flush", "sync"]);
  assert.equal(SYNC_MESSAGE, "Edits from LibrePaper");
  assert.deepEqual(asked(calls, "gitSync"), [["gitSync", "f1", "Edits from LibrePaper"]]);
  assert.deepEqual(git.state.steps.map((step) => step.text), ["Committed 1a2b3c4.", "Pulled and merged.", "Pushed."]);
  assert.ok(git.state.steps.every((step) => !step.failed));
  assert.equal(git.state.refusal, null);
  assert.equal(asked(calls, "gitStatus").length, 1);

  await git.sync("  Reword the greeting ");
  assert.deepEqual(asked(calls, "gitSync")[1], ["gitSync", "f1", "Reword the greeting"]);
}

// A clean tree commits nothing, and a branch with nothing to send pushes nothing.
{
  const steps = [{ step: "commit", status: "nothing" }, { step: "pull", status: "up-to-date", conflicts: [] }, { step: "push", status: "up-to-date" }];
  const { api } = routes({ gitStatus: repo, gitSync: { steps } });
  const git = createLocalGit({ id: "f1", api });
  await git.sync();
  assert.deepEqual(git.state.steps.map((step) => step.text), ["Nothing to commit.", "Already up to date.", "Nothing to push."]);
}

// Sync stops at the first step that fails, says which, and shows that step as its own action would.
{
  const cases = [
    {
      steps: [{ step: "commit", commit: "1a2b3c4" }, { step: "pull", reason: "dirty", paths: ["refs.bib"] }],
      check: (state) => { assert.deepEqual(state.refusal, { reason: "dirty", paths: ["refs.bib"] }); assert.match(state.steps[1].text, /Stopped at pull/); },
    },
    {
      steps: [{ step: "commit", status: "nothing" }, { step: "pull", status: "merged" }, { step: "push", reason: "behind" }],
      check: (state) => { assert.deepEqual(state.refusal, { reason: "behind" }); assert.match(state.steps[2].text, /Stopped at push/); },
    },
    {
      steps: [{ step: "commit", status: "nothing" }, { step: "pull", status: "up-to-date" }, { step: "push", reason: "network", detail: "timed out" }],
      check: (state) => { assert.deepEqual(state.refusal, { reason: "network", detail: "timed out" }); },
    },
    {
      steps: [{ step: "commit", status: "nothing" }, { step: "pull", status: "conflicts", conflicts: ["intro.tex"] }],
      check: (state) => { assert.deepEqual(state.conflicts, ["intro.tex"]); assert.equal(state.steps[1].failed, true); assert.equal(state.steps.length, 2, "push never ran"); },
    },
    {
      steps: [{ step: "commit", status: "nothing" }, { step: "pull", status: "up-to-date" }, { step: "push", reason: "hook", detail: "pre-push hook failed" }],
      check: (state) => { assert.equal(state.error, "pre-push hook failed"); assert.equal(state.steps[2].failed, true); },
    },
  ];
  for (const { steps, check } of cases) {
    const { api } = routes({ gitStatus: repo, gitSync: { steps } });
    const git = createLocalGit({ id: "f1", api });
    await git.sync();
    check(git.state);
    assert.equal(git.state.steps.at(-1).failed, true, "the last step is the one that stopped it");
    assert.equal(git.state.busy, "");
  }
}

// A sign-in that fails in a Sync repeats the Sync with the same message.
{
  const scene = { answer: { steps: [{ step: "commit", commit: "1a2b3c4" }, { step: "pull", status: "up-to-date" }, { step: "push", reason: "auth", detail: "Permission denied" }] } };
  const { api, calls } = routes({ gitStatus: repo, gitSync: () => scene.answer });
  const git = createLocalGit({ id: "f1", api });
  await git.sync("Reword the greeting");
  assert.deepEqual(git.state.auth, { detail: "Permission denied", action: "sync" });
  assert.equal(git.state.steps.length, 3, "the steps that ran stay on screen");
  scene.answer = { steps: [{ step: "commit", status: "nothing" }, { step: "pull", status: "up-to-date" }, { step: "push" }] };
  await git.retrySignIn();
  assert.deepEqual(asked(calls, "gitSync")[1], ["gitSync", "f1", "Reword the greeting"]);
  assert.equal(git.state.auth, null);
}

// A Sync with no identity asks for one, and saving it runs the Sync again.
{
  let identified = false;
  const { api, calls } = routes({
    gitStatus: repo,
    gitSync: () => (identified
      ? { steps: [{ step: "commit", commit: "abcdef0" }, { step: "pull", status: "up-to-date" }, { step: "push" }] }
      : { steps: [{ step: "commit", reason: "identity" }] }),
    gitIdentity: () => { identified = true; },
  });
  const git = createLocalGit({ id: "f1", api });
  await git.sync("Reword the greeting");
  assert.deepEqual(git.state.identity, { then: "sync", message: "Reword the greeting" });
  assert.equal(git.state.error, "");
  await git.saveIdentity("Anne", "anne@example.org");
  assert.deepEqual(asked(calls, "gitSync")[1], ["gitSync", "f1", "Reword the greeting"]);
  assert.equal(git.state.identity, null);
  assert.deepEqual(git.state.steps.map((step) => step.text), ["Committed abcdef0.", "Already up to date.", "Pushed."]);
}

// A Sync the desktop app answers with nothing is said, so is a failure that is not a step, and a folder that could not be saved is not synced.
{
  const { api } = routes({ gitStatus: repo, gitSync: { steps: [] } });
  const git = createLocalGit({ id: "f1", api });
  await git.sync();
  assert.match(git.state.error, /did not say/);
  const stopped = createLocalGit({ id: "f1", api: routes({ gitStatus: repo, gitSync: refused(500, { error: "git is not installed" }) }).api });
  await stopped.sync();
  assert.equal(stopped.state.error, "git is not installed");
  const unsaved = routes({ gitStatus: repo, gitSync: { steps: [] } });
  const blocked = createLocalGit({ id: "f1", api: unsaved.api, flush: async () => false });
  await blocked.sync();
  assert.equal(asked(unsaved.calls, "gitSync").length, 0, "a folder that could not be saved is not synced");
}

// One action at a time: a second press while the first is running does nothing.
{
  const gates = [];
  const { api, calls } = routes({ gitStatus: repo, gitPush: () => new Promise((resolve) => gates.push(resolve)) });
  const git = createLocalGit({ id: "f1", api });
  const first = git.push();
  await idle();
  assert.equal(git.state.busy, "push");
  await git.push();
  await git.pull();
  gates.shift()();
  await first;
  assert.equal(asked(calls, "gitPush").length, 1);
  assert.equal(asked(calls, "gitPull").length, 0);
  assert.equal(git.state.busy, "");
}

// A new action clears what the last one said.
{
  const answers = [refused(409, { reason: "behind" }), undefined];
  const { api } = routes({ gitStatus: repo, gitPush: () => answers.shift() });
  const git = createLocalGit({ id: "f1", api });
  await git.push();
  assert.deepEqual(git.state.refusal, { reason: "behind" });
  await git.push();
  assert.equal(git.state.refusal, null);
  assert.equal(git.state.steps[0].text, "Pushed.");
  git.dismiss();
  assert.deepEqual(git.state.steps, [{ step: "push", text: "Pushed." }], "dismissing a refusal leaves what was done");
}

// A panel that goes away stops the state from changing.
{
  const gates = [];
  const { api } = routes({ gitStatus: repo, gitPush: () => new Promise((resolve) => gates.push(resolve)) });
  const git = createLocalGit({ id: "f1", api });
  const running = git.push();
  await idle();
  git.dispose();
  gates.shift()();
  await running;
  assert.equal(git.state.steps.length, 0);
}

console.log("local-git: status, init, commit and identity, branches, remote, fetch, pull, push and sync, with their refusals");
