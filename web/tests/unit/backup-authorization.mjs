import assert from "node:assert/strict";
import * as local from "../../src/lib/companion/client.js";

const response = (body, status = 200) => ({
  status,
  ok: status >= 200 && status < 300,
  json: async () => body,
  clone() { return response(body, status); },
});

const pairingKey = "librepaper-local-connections";
let storage;
let requests;
let onAuthorize;

function replacePairing(token) {
  storage.set(pairingKey, JSON.stringify({
    "https://papers.example": { token, expires: 1_000_000, instance: "companion" },
  }));
}

function setup() {
  local._testing.reset();
  storage = new Map();
  requests = [];
  onAuthorize = null;
  replacePairing("pairing-original");
  local._testing.inject({
    storage: {
      getItem: (key) => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: (key) => storage.delete(key),
    },
    location: () => ({ href: "https://papers.example/docs/paper" }),
    fetch: async (url, init) => {
      const body = init.body ? JSON.parse(init.body) : {};
      const request = { url, ...init, body };
      requests.push(request);
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
const success = await local.authorizeBackups("account-a", async (code, pairingCurrent) => {
  order.push("approve");
  assert.equal(code, "ABCD-EFGH");
  assert.equal(typeof pairingCurrent, "function", "the callback receives the live pairing guard");
  assert.equal(pairingCurrent(), true);
}, () => true);
order.push("complete");
assert.deepEqual(order, ["approve", "complete"]);
assert.equal(success.status, "authorized");
assert.deepEqual(requests.map(({ url }) => url.split("/").at(-1)), ["authorize", "complete"]);
assert.ok(requests.every(({ headers }) => headers.Authorization === "Bearer pairing-original"), "both calls use the captured pairing token");
assert.deepEqual(requests[0].body, { account_id: "account-a" });
assert.deepEqual(requests[1].body, { account_id: "account-a", authorization_id: "authorization-1" });

setup();
await assert.rejects(
  local.authorizeBackups("account-a", async () => { throw new Error("approval refused"); }, () => true),
  /approval refused/,
);
assert.equal(requests.length, 1, "a failed browser approval never completes companion authorization");

setup();
let callbackPairingGuard;
let approved = false;
await assert.rejects(
  local.authorizeBackups("account-a", async (_code, pairingCurrent) => {
    callbackPairingGuard = pairingCurrent;
    replacePairing("pairing-replaced-during-callback");
    assert.equal(pairingCurrent(), false, "the callback guard reflects a pairing replacement during browser identity checks");
  }, () => true),
  (error) => error.name === "Canceled",
);
assert.equal(callbackPairingGuard(), false);
assert.equal(requests.length, 1, "a replaced pairing cannot complete authorization");

setup();
onAuthorize = () => replacePairing("pairing-replaced-before-code");
approved = false;
await assert.rejects(
  local.authorizeBackups("account-a", async () => { approved = true; }, () => true),
  (error) => error.name === "Canceled",
);
assert.equal(approved, false, "a pairing replaced during start stops before browser approval");
assert.equal(requests.length, 1, "a replaced start pairing never reaches completion");

setup();
approved = false;
await assert.rejects(
  local.authorizeBackups("account-a", async () => { approved = true; }, () => false),
  (error) => error.name === "Canceled",
);
assert.equal(approved, false, "a changed account/UI scope after start stops before browser approval");
assert.equal(requests.length, 1, "a changed account/UI scope never reaches completion");

local._testing.reset();
console.log("backup-authorization: order, approval failure, and live pairing/UI scope guards passed");
