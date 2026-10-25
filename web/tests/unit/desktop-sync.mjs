// The sync calls of a folder project and "Keep a folder on this computer",
// using the production client and fake HTTP: which route each one reaches,
// with which method and body, and how a refusal arrives.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import { folderProjectHref, keepFolderProject } from "../../src/lib/desktop/projects.js";
import { LOGIN_PENDING, syncLink, syncLogin, syncLoginComplete, syncPublish, syncPull, syncPush, syncStatus, syncUnlink } from "../../src/lib/desktop/sync.js";

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

const status = { signed_in: true, linked: true, site: "https://app.example", slug: "paper", pulled: 4, changes: { here: 2, web: false } };
answer = () => response(200, status);
assert.deepEqual(await syncStatus("f1"), status);
assert.equal(last().method, "GET");
assert.ok(last().url.endsWith("projects/f1/sync"));

// A folder that holds a .git says so, and the panel stops offering the commands.
answer = () => response(200, { ...status, git: true });
assert.equal((await syncStatus("f1")).git, true);

// Signing in is two calls: a code handed out at once, then the finish.
answer = () => response(200, { authorization_id: "auth-1", user_code: "ABCD-EFGH", verification_url: "https://app.example/device" });
assert.deepEqual(await syncLogin("f1"), { authorization_id: "auth-1", user_code: "ABCD-EFGH", verification_url: "https://app.example/device" });
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/sync/login"));
assert.deepEqual(last().body, {});

answer = () => response(200, status);
assert.deepEqual(await syncLoginComplete("f1", "auth-1"), status);
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/sync/login/complete"));
assert.deepEqual(last().body, { authorization_id: "auth-1" });

// A code that is not approved yet is a 400 with the words the controller waits on.
answer = () => response(400, { error: "sign in and approve the code, then retry" });
await assert.rejects(syncLoginComplete("f1", "auth-1"), (error) => error.status === 400 && error.body.error === LOGIN_PENDING);

answer = () => response(204, null);
await syncLink("f1", "https://app.example/docs/paper");
assert.ok(last().url.endsWith("projects/f1/sync/link"));
assert.deepEqual(last().body, { url: "https://app.example/docs/paper" });

await syncPublish("f1", "My paper");
assert.ok(last().url.endsWith("projects/f1/sync/publish"));
assert.deepEqual(last().body, { title: "My paper" });

await syncUnlink("f1");
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/f1/sync/unlink"));

// A pull sends the answers to a conflict, one path at a time, and nothing else.
const pulled = { written: ["intro.tex"], trashed: [], kept: [], conflicts: ["a.tex"], trimmed: false };
answer = () => response(200, pulled);
assert.deepEqual(await syncPull("f1"), pulled);
assert.ok(last().url.endsWith("projects/f1/sync/pull"));
assert.deepEqual(last().body, {});
await syncPull("f1", { mine: ["a.tex"] });
assert.deepEqual(last().body, { mine: ["a.tex"] });
await syncPull("f1", { theirs: ["a.tex"] });
assert.deepEqual(last().body, { theirs: ["a.tex"] });

// A pull without a sign-in is a 401 with the service's own words.
answer = () => response(401, { error: "sign in" });
await assert.rejects(syncPull("f1"), (error) => error.status === 401 && error.message === "sign in");

// A push says what it sent, or that there was nothing to send.
answer = () => response(200, { label: { sha: "abc", sequence: 5 }, files: 2 });
assert.deepEqual(await syncPush("f1", "Fix the abstract"), { label: { sha: "abc", sequence: 5 }, files: 2 });
assert.ok(last().url.endsWith("projects/f1/sync/push"));
assert.deepEqual(last().body, { message: "Fix the abstract" });
answer = () => response(200, { nothing: true });
assert.deepEqual(await syncPush("f1", "m"), { nothing: true });

// A refused push carries its reason and paths on the error.
answer = () => response(409, { reason: "changed", paths: ["intro.tex"] });
await assert.rejects(syncPush("f1", "m"), (error) => error.status === 409 && error.body.reason === "changed" && error.body.paths[0] === "intro.tex");
answer = () => response(409, { reason: "suggestion", path: "intro.tex" });
await assert.rejects(syncPush("f1", "m"), (error) => error.status === 409 && error.body.reason === "suggestion" && error.body.path === "intro.tex");

// An id is one path segment, whatever it holds.
answer = () => response(200, status);
await syncStatus("a/b");
assert.ok(last().url.endsWith("projects/a%2Fb/sync"));

// Keeping a web project in a folder sends the project page and gets the folder project back.
const project = { id: "f2", project: "local-f2", name: "paper", root_name: "paper" };
answer = () => response(200, project);
assert.deepEqual(await keepFolderProject("https://app.example/docs/paper"), project);
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("projects/keep"));
assert.deepEqual(last().body, { url: "https://app.example/docs/paper" });
assert.equal(folderProjectHref("f2"), "/reader.html?local=f2");

// Cancelling the chooser is no project.
answer = () => response(204, null);
assert.equal(await keepFolderProject("https://app.example/docs/paper"), null);

// A folder with files in it is refused in the service's words.
answer = () => response(409, { error: "choose an empty folder" });
await assert.rejects(keepFolderProject("https://app.example/docs/paper"), (error) => error.status === 409 && error.message === "choose an empty folder");

console.log("desktop-sync: sync routes, pull answers, push refusals and keeping a folder");
