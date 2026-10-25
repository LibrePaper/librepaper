// A folder project's git calls, using the production client and fake HTTP:
// which route each one reaches, with which method and body, and how a refusal
// arrives. The desktop app runs git; this is only what the page sends.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import {
  gitBranch, gitBranches, gitCommit, gitDiff, gitFetch, gitIdentity, gitInit, gitPull, gitPush, gitRemote, gitStatus, gitSwitch, gitSync,
} from "../../src/lib/desktop/git.js";

globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
client._testing.reset();

function response(status, body) {
  return { ok: status >= 200 && status < 300, status, json: async () => body, clone() { return response(status, body); } };
}
const calls = [];
let answer;
client._testing.inject({
  fetch: async (url, init = {}) => {
    calls.push({ url, method: init.method, body: init.body ? JSON.parse(init.body) : undefined });
    return answer();
  },
});
const last = () => calls.at(-1);

// The status is the repository as git sees it.
const status = {
  repository: true, git: { found: true, version: "2.43.0" }, branch: "main", upstream: "origin/main", ahead: 1, behind: 0,
  remotes: [{ name: "origin", url: "https://github.com/you/paper.git" }],
  changes: [{ path: "intro.tex", kind: "modified" }, { path: "b.tex", kind: "renamed", from: "a.tex" }],
  merging: false, identity: true,
};
answer = () => response(200, status);
assert.deepEqual(await gitStatus("f1"), status);
assert.equal(last().method, "GET");
assert.ok(last().url.endsWith("projects/f1/git"));

// Initializing sends nothing but the route.
answer = () => response(204, null);
await gitInit("f1");
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/git/init"));
assert.equal(last().body, undefined);

// A diff names its file in the query, escaped.
answer = () => response(200, { head: "old\n", work: "new\n", binary: false });
assert.deepEqual(await gitDiff("f1", "chapters/a b.tex"), { head: "old\n", work: "new\n", binary: false });
assert.equal(last().method, "GET");
assert.ok(last().url.endsWith("projects/f1/git/diff?path=chapters%2Fa%20b.tex"));
answer = () => response(200, { head: null, work: "new\n", binary: false });
assert.equal((await gitDiff("f1", "new.tex")).head, null, "a file that was not in the last commit has no head");

// A commit sends the message, and the paths only when some were left out.
answer = () => response(200, { commit: "1a2b3c4d5e6f" });
assert.deepEqual(await gitCommit("f1", "Fix the abstract"), { commit: "1a2b3c4d5e6f" });
assert.ok(last().url.endsWith("projects/f1/git/commit"));
assert.deepEqual(last().body, { message: "Fix the abstract" });
await gitCommit("f1", "Only one", ["intro.tex"]);
assert.deepEqual(last().body, { message: "Only one", paths: ["intro.tex"] });

// A missing identity is a 409 with its reason on the error.
answer = () => response(409, { reason: "identity" });
await assert.rejects(gitCommit("f1", "m"), (error) => error.status === 409 && error.body.reason === "identity");

answer = () => response(204, null);
await gitIdentity("f1", "Anne Example", "anne@example.org");
assert.ok(last().url.endsWith("projects/f1/git/identity"));
assert.deepEqual(last().body, { name: "Anne Example", email: "anne@example.org" });

// Branches: the list, switching to one, and making one.
answer = () => response(200, { current: "main", branches: ["main", "draft"] });
assert.deepEqual(await gitBranches("f1"), { current: "main", branches: ["main", "draft"] });
assert.equal(last().method, "GET");
assert.ok(last().url.endsWith("projects/f1/git/branches"));
answer = () => response(204, null);
await gitSwitch("f1", "draft");
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/git/switch"));
assert.deepEqual(last().body, { branch: "draft" });
await gitBranch("f1", "review");
assert.ok(last().url.endsWith("projects/f1/git/branch"));
assert.deepEqual(last().body, { branch: "review" });

// A remote is a name and an address.
await gitRemote("f1", "origin", "git@github.com:you/paper.git");
assert.ok(last().url.endsWith("projects/f1/git/remote"));
assert.deepEqual(last().body, { name: "origin", url: "git@github.com:you/paper.git" });
answer = () => response(400, { error: "That remote URL cannot be used: it contains a password." });
await assert.rejects(gitRemote("f1", "origin", "https://u:p@host/r.git"), (error) => error.status === 400 && /password/.test(error.message));

// A fetch says how far the branch is from its upstream.
answer = () => response(200, { ahead: 2, behind: 1 });
assert.deepEqual(await gitFetch("f1"), { ahead: 2, behind: 1 });
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/git/fetch"));

// A pull answers how it went, and refuses over uncommitted files with the paths.
answer = () => response(200, { status: "conflicts", conflicts: ["a.tex"] });
assert.deepEqual(await gitPull("f1"), { status: "conflicts", conflicts: ["a.tex"] });
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/git/pull"));
answer = () => response(409, { reason: "dirty", paths: ["intro.tex"] });
await assert.rejects(gitPull("f1"), (error) => error.status === 409 && error.body.reason === "dirty" && error.body.paths[0] === "intro.tex");

// A push refuses when the remote moved, and says why it failed to reach it.
answer = () => response(204, null);
await gitPush("f1");
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/git/push"));
answer = () => response(409, { reason: "behind" });
await assert.rejects(gitPush("f1"), (error) => error.body.reason === "behind");
answer = () => response(409, { reason: "auth", detail: "Permission denied (publickey)." });
await assert.rejects(gitPush("f1"), (error) => error.body.reason === "auth" && /publickey/.test(error.body.detail));
answer = () => response(409, { reason: "network", detail: "Could not resolve host" });
await assert.rejects(gitPush("f1"), (error) => error.body.reason === "network");

// A sync sends a message when it has one, and answers the steps it ran.
const steps = [{ step: "commit", commit: "1a2b3c4" }, { step: "pull", status: "up-to-date", conflicts: [] }, { step: "push" }];
answer = () => response(200, { steps });
assert.deepEqual(await gitSync("f1", "Edits from LibrePaper"), { steps });
assert.ok(last().url.endsWith("projects/f1/git/sync"));
assert.deepEqual(last().body, { message: "Edits from LibrePaper" });
await gitSync("f1");
assert.deepEqual(last().body, {}, "no message, no field");

// A sync that stopped may come as a refusal; its steps are still the answer.
const stopped = [{ step: "commit", commit: "1a2b3c4" }, { step: "push", reason: "behind" }];
answer = () => response(409, { steps: stopped });
assert.deepEqual(await gitSync("f1"), { steps: stopped });
// Any other refusal is still a failure.
answer = () => response(500, { error: "git failed" });
await assert.rejects(gitSync("f1"), (error) => error.status === 500 && error.message === "git failed");

// An id is one path segment, whatever it holds.
answer = () => response(200, status);
await gitStatus("a/b");
assert.ok(last().url.endsWith("projects/a%2Fb/git"));

console.log("desktop-git: every git route, its method and body, and how a refusal arrives");
