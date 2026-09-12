import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import * as local from "../../src/lib/latex/local.js";

const response = (status, body) => ({ status, ok: status >= 200 && status < 300,
  json: async () => body, clone() { return response(status, body); } });
let now, link, storage, claimBody, attempts;
function setup(claim) {
  local._testing.reset();
  now = 0; link = ""; storage = new Map(); attempts = 0;
  globalThis.window = { document: {
    createElement: () => ({ remove() {} }),
    body: { appendChild(frame) { link = frame.src; } },
  } };
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    now: () => now,
    wait: async () => { now += 700; },
    fetch: async (url, init) => {
      if (url.includes("connect/claim")) { claimBody = JSON.parse(init.body); return claim(++attempts); }
      if (url.includes("health")) return response(200, { service: "librepaper-local", protocol: [1], instance: "restart" });
      if (url.includes("capabilities")) return response(200, { tools: {} });
      throw new Error(`Unexpected request ${url}`);
    },
  });
  local.configure({ origin: "https://papers.example", project: "paper" });
}

setup((n) => {
  if (n === 1) throw new TypeError("Failed to fetch"); // cold start
  return n === 2 ? response(404, {}) : n === 3 ? response(202, {}) : response(200, { token: "scoped-token", expires: 1000000, instance: "restart" });
});
await local.connectViaApp({ timeoutMs: 10000 });
const url = new URL(link);
assert.equal(url.protocol, "librepaper:");
assert.equal(url.hostname, "connect");
assert.equal(url.searchParams.get("origin"), "https://papers.example");
assert.equal(url.searchParams.get("challenge"), createHash("sha256").update(claimBody.verifier).digest("hex"));
assert.equal(url.searchParams.get("request"), claimBody.request);
assert.ok(claimBody.request.length >= 32);
assert.ok(!link.includes(claimBody.verifier));
assert.ok(!link.includes("scoped-token"));
assert.equal(attempts, 4);
assert.equal(local.status().state, "connected");
await local.connectViaApp({ timeoutMs: 10000 });
assert.equal(link, "librepaper://launch", "existing permission launches without new consent");
assert.equal(attempts, 4, "restart reuses stored token");

setup(() => response(403, {}));
await assert.rejects(local.connectViaApp(), /expired or was refused/);
assert.equal(attempts, 1);
assert.ok(!storage.has("librepaper-local-pairings"));

setup(() => { local.configure({ project: "another-paper" }); return response(200, { token: "wrong-project", expires: 100000 }); });
await assert.rejects(local.connectViaApp(), /document or companion address changed/);
assert.ok(!storage.has("librepaper-local-pairings"));

setup(() => response(202, {}));
await assert.rejects(local.connectViaApp({ timeoutMs: 1500 }), /did not connect/);
assert.ok(!storage.has("librepaper-local-pairings"));
local._testing.reset();
delete globalThis.window;
console.log("local-companion: cold launch, verifier isolation, pending, restart, rejection, scope change and timeout passed");
