import assert from "node:assert/strict";
import * as local from "../../src/lib/desktop/client.js";

const response = (body, status = 200) => ({
  status,
  ok: status >= 200 && status < 300,
  json: async () => body,
  clone() { return response(body, status); },
});

let requests;
let onAuthorize;

function setup() {
  requests = [];
  onAuthorize = null;
  globalThis.window = { __LIBREPAPER_DESKTOP__: { base: "lp-local://localhost/", version: "test" } };
  local._testing.reset();
  local._testing.inject({
    fetch: async (url, init) => {
      const body = init.body ? JSON.parse(init.body) : {};
      requests.push({ url, ...init, body });
      if (url.endsWith("backups/authorize")) {
        await onAuthorize?.();
        return response({ user_code: "ABCD-EFGH", authorization_id: "authorization-1" });
      }
      if (url.endsWith("backups/authorize/complete")) return response({ status: "authorized" });
      throw new Error(`Unexpected request: ${url}`);
    },
  });
  local.configure({ origin: "https://papers.example", project: "paper" });
}

setup();
const order = [];
const success = await local.authorizeBackups("account-a", async (code, stillCurrent) => {
  order.push("approve");
  assert.equal(code, "ABCD-EFGH");
  assert.equal(typeof stillCurrent, "function", "the callback receives the live scope guard");
  assert.equal(stillCurrent(), true);
}, () => true);
order.push("complete");
assert.deepEqual(order, ["approve", "complete"]);
assert.equal(success.status, "authorized");
assert.deepEqual(requests.map(({ url }) => url), [
  "lp-local://localhost/librepaper/local/backups/authorize",
  "lp-local://localhost/librepaper/local/backups/authorize/complete",
]);
assert.ok(requests.every(({ headers }) => !("Authorization" in headers)), "the page sends no credential");
assert.deepEqual(requests[0].body, { account_id: "account-a" });
assert.deepEqual(requests[1].body, { account_id: "account-a", authorization_id: "authorization-1" });

setup();
await assert.rejects(
  local.authorizeBackups("account-a", async () => { throw new Error("approval refused"); }, () => true),
  /approval refused/,
);
assert.equal(requests.length, 1, "a failed browser approval never completes desktop app authorization");

setup();
let current = true;
await assert.rejects(
  local.authorizeBackups("account-a", async (_code, stillCurrent) => {
    current = false;
    assert.equal(stillCurrent(), false, "the callback guard reflects a scope change during browser identity checks");
  }, () => current),
  (error) => error.name === "Canceled",
);
assert.equal(requests.length, 1, "a changed scope cannot complete authorization");

setup();
let approved = false;
await assert.rejects(
  local.authorizeBackups("account-a", async () => { approved = true; }, () => false),
  (error) => error.name === "Canceled",
);
assert.equal(approved, false, "a changed account/UI scope after start stops before browser approval");
assert.equal(requests.length, 1, "a changed account/UI scope never reaches completion");

local._testing.reset();
delete globalThis.window;
console.log("backup-authorization: order, approval failure, and live scope guards passed");
