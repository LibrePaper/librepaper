import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import * as local from "../../src/lib/companion/client.js";

const response = (status, body) => ({ status, ok: status >= 200 && status < 300,
  json: async () => body, clone() { return response(status, body); } });

let now, link, popup, storage, claimBody, attempts;
function inject(claim) {
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    now: () => now,
    wait: async () => { now += 700; },
    location: () => ({ href: "https://papers.example/docs/paper" }),
    launchLink: (url) => { link = url; },
    openPopup: () => { popup = null; return popup; },
    fetch: async (url, init) => {
      if (url.includes("connect/claim")) { claimBody = JSON.parse(init.body); return claim(++attempts); }
      if (url.includes("health")) return response(200, { service: "librepaper-local", protocol: [2], instance: "restart" });
      if (url.includes("capabilities")) return response(200, { tools: {} });
      throw new Error(`Unexpected request ${url}`);
    },
  });
}

// A fresh attempt: no pairing yet, and the status this tab last probed is
// not "unauthorized" or "reachable", so `connectApp` goes straight to the
// `librepaper://connect` link rather than trying a popup first.
function setup(claim) {
  local._testing.reset();
  now = 0; link = ""; popup = null; storage = new Map(); attempts = 0;
  inject(claim);
  local.configure({ origin: "https://papers.example", project: "paper" });
}

setup((n) => {
  if (n === 1) throw new TypeError("Failed to fetch"); // cold start
  return n === 2 ? response(404, {}) : n === 3 ? response(202, {}) : response(200, { token: "scoped-token", expires: 1000000, instance: "restart" });
});
await local.connectApp({ timeoutMs: 10000 });
const url = new URL(link);
assert.equal(url.protocol, "librepaper:");
assert.equal(url.hostname, "connect");
assert.equal(url.searchParams.get("origin"), "https://papers.example");
assert.equal(url.searchParams.get("challenge"), createHash("sha256").update(claimBody.verifier).digest("hex"));
assert.equal(url.searchParams.get("request"), claimBody.request);
assert.equal(url.searchParams.get("return"), "https://papers.example/docs/paper");
assert.ok(claimBody.request.length >= 32);
assert.ok(!link.includes(claimBody.verifier));
assert.ok(!link.includes("scoped-token"));
assert.equal(attempts, 4);
assert.equal(local.status().state, "connected");

// A restart before reconnecting: the stored pairing survives (it is read
// straight from storage), but this tab's own memory of being connected does
// not, so the launch link fires again rather than being skipped.
const priorLink = link;
now = 0; link = ""; attempts = 0;
local._testing.reset();
inject(() => response(200, {}));
local.configure({ origin: "https://papers.example", project: "paper" });
await local.connectApp({ timeoutMs: 10000 });
assert.notEqual(link, priorLink, "a fresh attempt mints a fresh link");
const relaunch = new URL(link);
assert.equal(relaunch.protocol, "librepaper:");
assert.equal(relaunch.hostname, "launch");
assert.equal(relaunch.searchParams.get("origin"), "https://papers.example");
assert.equal(relaunch.searchParams.get("project"), null, "a launch link carries exactly origin, request and return");
assert.equal(attempts, 0, "an already-paired reconnect polls the probe rather than claiming again");
assert.equal(local.status().state, "connected");

// Already connected: nothing to relaunch for, so `connectApp` resolves at
// once without firing another link at all.
link = "";
await local.connectApp({ timeoutMs: 10000 });
assert.equal(link, "", "an already-connected browser fires no link");

setup(() => response(403, {}));
await assert.rejects(local.connectApp(), /expired or was refused/);
assert.equal(attempts, 1);
assert.ok(!storage.has("librepaper-local-pairings"));

setup(() => { local.configure({ project: "another-paper" }); return response(200, { token: "wrong-project", expires: 100000 }); });
await assert.rejects(local.connectApp(), /document changed/);
assert.ok(!storage.has("librepaper-local-pairings"));

setup(() => response(202, {}));
await assert.rejects(local.connectApp({ timeoutMs: 1500 }), /did not connect/);
assert.ok(!storage.has("librepaper-local-pairings"));
local._testing.reset();
console.log("local-companion: cold launch, verifier isolation, restart, already-connected, rejection, scope change and timeout passed");

// Fragment intake: the OS link handler's only channel back to this page is a
// top-level navigation carrying the real port in the hash, matched against
// the pending request this same tab wrote before dispatching the link. The
// hash is stripped whatever it says.
function fragmentSetup(hash, pending) {
  local._testing.reset();
  const map = new Map();
  if (pending !== undefined) map.set("librepaper-local-pending", JSON.stringify(pending));
  let replaced = null;
  local._testing.inject({
    storage: { getItem: (k) => map.get(k) ?? null, setItem: (k, v) => map.set(k, v), removeItem: (k) => map.delete(k) },
    now: () => 1000,
    location: () => ({ hash }),
    replaceHash: (value) => { replaced = value; },
  });
  local._testing.intakeFragment();
  return { map, replaced };
}

const pending = { request: "req-1", expires: 2000 };
const encode = (address) => encodeURIComponent(address);
const goodHash = `#librepaper-local=${encode("http://127.0.0.1:8763/")}&librepaper-request=req-1&other=kept`;

{
  const { map, replaced } = fragmentSetup(goodHash, pending);
  assert.equal(map.get("librepaper-local-address"), "http://127.0.0.1:8763/", "a matching loopback address with an explicit port is accepted");
  assert.equal(map.has("librepaper-local-pending"), false, "the pending entry is cleared");
  assert.equal(replaced, "other=kept", "the two params are stripped, the rest of the hash kept");
}
{
  const { map, replaced } = fragmentSetup(goodHash.replace("req-1", "req-2"), pending);
  assert.equal(map.has("librepaper-local-address"), false, "a request that does not match the pending one is rejected");
  assert.equal(replaced, "other=kept", "the hash is stripped regardless");
}
{
  const { map } = fragmentSetup(goodHash, { request: "req-1", expires: 500 });
  assert.equal(map.has("librepaper-local-address"), false, "an expired pending request is rejected");
}
{
  const nonLoopback = `#librepaper-local=${encode("http://example.com:8763/")}&librepaper-request=req-1`;
  const { map } = fragmentSetup(nonLoopback, pending);
  assert.equal(map.has("librepaper-local-address"), false, "a non-loopback host is rejected");
}
{
  const httpsAddress = `#librepaper-local=${encode("https://127.0.0.1:8763/")}&librepaper-request=req-1`;
  const { map } = fragmentSetup(httpsAddress, pending);
  assert.equal(map.has("librepaper-local-address"), false, "https is rejected");
}
{
  const withPath = `#librepaper-local=${encode("http://127.0.0.1:8763/extra")}&librepaper-request=req-1`;
  const { map } = fragmentSetup(withPath, pending);
  assert.equal(map.has("librepaper-local-address"), false, "a path beyond / is rejected");
}
{
  const withCredentials = `#librepaper-local=${encode("http://user:pass@127.0.0.1:8763/")}&librepaper-request=req-1`;
  const { map } = fragmentSetup(withCredentials, pending);
  assert.equal(map.has("librepaper-local-address"), false, "credentials in the address are rejected");
}
{
  const { map, replaced } = fragmentSetup("#nothing=here", pending);
  assert.equal(map.has("librepaper-local-address"), false);
  assert.equal(replaced, null, "a hash naming neither param is left untouched");
}
local._testing.reset();
console.log("local-companion: fragment intake accepts a matching loopback address and rejects a mismatched, expired, non-loopback, https, pathed or credentialed one, stripping the hash every time it names either param");
