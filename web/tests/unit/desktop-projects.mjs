// The landing page's folder-project calls, using the production client and fake HTTP.
import assert from "node:assert/strict";
import * as client from "../../src/lib/desktop/client.js";
import { openFolderProject, folderProjects, forgetFolderProject } from "../../src/lib/desktop/projects.js";

globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
client._testing.reset();

function response(status, body) {
  return { ok: status >= 200 && status < 300, status, json: async () => body, clone() { return response(status, body); } };
}
const calls = [];
let answer;
client._testing.inject({ fetch: async (url, init = {}) => { calls.push({ url, method: init.method }); return answer(); } });

answer = () => response(200, { id: "f1", project: "local-f1", name: "thesis", root_name: "thesis" });
assert.deepEqual(await openFolderProject(), { id: "f1", project: "local-f1", name: "thesis", root_name: "thesis" });
assert.equal(calls.at(-1).method, "POST");
assert.ok(calls.at(-1).url.endsWith("projects/open"));

answer = () => response(204, null);
assert.equal(await openFolderProject(), null);

answer = () => response(409, { error: "A folder chooser is already open." });
await assert.rejects(openFolderProject(), (error) => error.status === 409 && /already open/.test(error.message));

const rows = [{ id: "f1", project: "local-f1", name: "thesis", opened_at: "2026-10-25T10:00:00Z", present: false }];
answer = () => response(200, rows);
assert.deepEqual(await folderProjects(), rows);
assert.equal(calls.at(-1).method, "GET");
assert.ok(calls.at(-1).url.endsWith("projects"));

answer = () => response(204, null);
await forgetFolderProject("f1");
assert.equal(calls.at(-1).method, "DELETE");
assert.ok(calls.at(-1).url.endsWith("projects/f1"));
