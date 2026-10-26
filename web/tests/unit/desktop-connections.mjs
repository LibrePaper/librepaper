// The connection calls of a web project, using the production client and fake
// HTTP: which route each one reaches, with which method and body, and how a
// refusal arrives.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import { connectionStatus, connectRepository, disconnectConnection, loginConnection, loginConnectionComplete, pullConnection, pushConnection, resolveConnection } from "../../src/lib/desktop/connections.js";

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
const project = "https://app.example/docs/paper";

// The status is read from the query, with the page address encoded.
const connected = { connected: true, signed_in: true, url: "https://github.com/you/paper.git", branch: "main", merging: null };
answer = () => response(200, connected);
assert.deepEqual(await connectionStatus(project), connected);
assert.equal(last().method, "GET");
assert.equal(last().url, `lp-local://localhost/librepaper/local/connections?project=${encodeURIComponent(project)}`);
assert.equal(last().body, undefined);

// Connecting sends the page address and the address, and answers the status.
answer = () => response(200, connected);
assert.deepEqual(await connectRepository(project, "https://github.com/you/paper.git"), connected);
assert.equal(last().method, "POST");
assert.ok(last().url.endsWith("librepaper/local/connections"));
assert.deepEqual(last().body, { project, url: "https://github.com/you/paper.git" });

// A repository already connected elsewhere is refused with its reason.
answer = () => response(409, { reason: "linked", error: "this repository is linked to another LibrePaper project" });
await assert.rejects(connectRepository(project, "https://github.com/you/paper.git"), (error) => error.status === 409 && error.body.reason === "linked");

// Pull sends the project alone and answers what it wrote, or the conflicts it stopped at.
answer = () => response(200, { status: connected, pushed: { files: 3 } });
assert.deepEqual(await pullConnection(project), { status: connected, pushed: { files: 3 } });
assert.ok(last().url.endsWith("librepaper/local/connections/pull"));
assert.deepEqual(last().body, { project });
answer = () => response(200, { stopped: "conflicts", status: { ...connected, merging: { side: "repository", conflicts: ["a.tex"] } } });
assert.equal((await pullConnection(project)).stopped, "conflicts");

// A pull that changed files on the web is refused with the paths.
answer = () => response(409, { reason: "changed", paths: ["intro.tex"] });
await assert.rejects(pullConnection(project), (error) => error.status === 409 && error.body.reason === "changed" && error.body.paths[0] === "intro.tex");

// Push sends the project alone, and a repository that moved ahead is refused as behind.
answer = () => response(200, { status: connected, pushed: "pushed" });
assert.deepEqual(await pushConnection(project), { status: connected, pushed: "pushed" });
assert.ok(last().url.endsWith("librepaper/local/connections/push"));
assert.deepEqual(last().body, { project });
answer = () => response(409, { reason: "behind" });
await assert.rejects(pushConnection(project), (error) => error.status === 409 && error.body.reason === "behind");

// Resolving a file sends its path and the side to keep.
answer = () => response(200, connected);
await resolveConnection(project, "a.tex", "repository");
assert.ok(last().url.endsWith("librepaper/local/connections/resolve"));
assert.deepEqual(last().body, { project, path: "a.tex", keep: "repository" });

// Disconnecting sends the project and answers the status.
answer = () => response(200, { connected: false, signed_in: true });
assert.deepEqual(await disconnectConnection(project), { connected: false, signed_in: true });
assert.ok(last().url.endsWith("librepaper/local/connections/disconnect"));
assert.deepEqual(last().body, { project });

// Signing in is two calls: a code handed out at once, then the finish with the authorization.
answer = () => response(200, { authorization_id: "auth-1", user_code: "ABCD-EFGH" });
assert.deepEqual(await loginConnection(project), { authorization_id: "auth-1", user_code: "ABCD-EFGH" });
assert.ok(last().url.endsWith("librepaper/local/connections/login"));
assert.deepEqual(last().body, { project });
answer = () => response(200, { connected: false, signed_in: true });
await loginConnectionComplete(project, "auth-1");
assert.ok(last().url.endsWith("librepaper/local/connections/login/complete"));
assert.deepEqual(last().body, { project, authorization_id: "auth-1" });

// A code not yet approved is a 400 that the controller waits on.
answer = () => response(400, { error: "sign in and approve the code, then retry" });
await assert.rejects(loginConnectionComplete(project, "auth-1"), (error) => error.status === 400);

console.log("desktop-connections: connection routes, bodies and refusals");
