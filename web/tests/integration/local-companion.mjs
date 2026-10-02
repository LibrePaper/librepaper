import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import * as local from "../../src/lib/companion/client.js";

const response = (status, body) => ({ status, ok: status >= 200 && status < 300,
  json: async () => body, clone() { return response(status, body); } });

let now, link, storage, claimBody, pairBody, attempts, companionUp, healthUp, permission, staleCapabilities;
function inject(claim) {
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    now: () => now,
    wait: async () => { now += 700; },
    location: () => ({ href: "https://papers.example/docs/paper" }),
    launchLink: (url) => { link = url; },
    localNetworkPermission: async () => permission,
    fetch: async (url, init) => {
      if (url.includes("pair/request")) {
        if (!companionUp) throw new TypeError("Failed to fetch");
        pairBody = JSON.parse(init.body); return response(202, { request: pairBody.request }); }
      if (url.includes("connect/claim")) { claimBody = JSON.parse(init.body); return claim(++attempts); }
      if (url.includes("health")) {
        if (!healthUp) throw new TypeError("Failed to fetch");
        return response(200, { service: "librepaper-local", protocol: [2], instance: "restart" }); }
      if (url.includes("capabilities")) {
        if (init.headers.Authorization === "Bearer stale-token") {
          if (staleCapabilities === "opaque") throw new TypeError("Failed to fetch");
          if (staleCapabilities === "unauthorized") return response(401, {});
        }
        return response(200, { tools: {} });
      }
      throw new Error(`Unexpected request ${url}`);
    },
  });
}

// A fresh attempt with no pairing yet and no companion answering: the direct
// ask fails, so `connectApp` falls back to the `librepaper://connect` link.
function setup(claim) {
  local._testing.reset();
  now = 0; link = ""; pairBody = null; storage = new Map(); attempts = 0; companionUp = false; healthUp = true; permission = "prompt"; staleCapabilities = null;
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

// A restart before reconnecting: a working stored pairing is revalidated and
// reused without prompting or launching the companion.
now = 0; link = ""; attempts = 0;
local._testing.reset();
inject(() => response(200, {}));
local.configure({ origin: "https://papers.example", project: "paper" });
await local.connectApp({ timeoutMs: 10000 });
assert.equal(link, "", "a working stored pairing needs no launch link");
assert.equal(attempts, 0, "an already-paired reconnect revalidates without claiming again");
assert.equal(local.status().state, "connected");

// A healthy app with a stale token can hide its 401 behind a browser CORS
// TypeError. Explicit Connect clears that unusable local token and starts a
// fresh native consent request in the same click.
for (const failure of ["opaque", "unauthorized"]) {
  setup((n) => (n === 1 ? response(202, {}) : response(200, { token: "fresh-token", expires: 1000000, instance: "restart" })));
  companionUp = true;
  staleCapabilities = failure;
  storage.set("librepaper-local-connections", JSON.stringify({
    "https://papers.example": { token: "stale-token", expires: 1000000, instance: "restart" },
  }));
  if (failure === "opaque") {
    const background = await local.probe({ force: true });
    assert.equal(background.state, "reachable", "a background credential-fetch failure keeps the companion reachable");
    assert.equal(JSON.parse(storage.get("librepaper-local-connections"))["https://papers.example"].token, "stale-token",
      "a transient background TypeError does not discard the stored pairing");
  }
  await local.connectApp({ timeoutMs: 10000 });
  assert.equal(link, "", `${failure}: reachable companion gets a direct consent request`);
  assert.equal(pairBody.origin, "https://papers.example");
  assert.equal(attempts, 2, `${failure}: fresh claim starts in the same Connect call`);
  assert.equal(local.status().state, "connected");
  assert.equal(JSON.parse(storage.get("librepaper-local-connections"))["https://papers.example"].token, "fresh-token");
}

// Already connected: nothing to relaunch for, so `connectApp` resolves at
// once without firing another link at all.
link = "";
await local.connectApp({ timeoutMs: 10000 });
assert.equal(link, "", "an already-connected browser fires no link");

// A browser denial is a permission problem, not evidence of a stale token.
// Keep the pairing and let the explicit Connect report the denial directly.
setup(() => response(200, { token: "unexpected-token", expires: 1000000, instance: "restart" }));
companionUp = true;
permission = "denied";
staleCapabilities = "opaque";
storage.set("librepaper-local-connections", JSON.stringify({
  "https://papers.example": { token: "stale-token", expires: 1000000, instance: "restart" },
}));
await assert.rejects(local.connectApp(), (error) => error.name === "Refused" && /blocked this site/.test(error.message));
assert.equal(JSON.parse(storage.get("librepaper-local-connections"))["https://papers.example"].token, "stale-token");
assert.equal(pairBody, null, "blocked permission does not request fresh consent");
assert.equal(link, "", "blocked permission does not launch the companion");

// A companion already answering: the page asks it directly even though this
// tab never probed it, the dialog appears with no link, and the claim picks
// up the token.
setup((n) => (n === 1 ? response(202, {}) : response(200, { token: "direct-token", expires: 1000000, instance: "restart" })));
companionUp = true;
await local.connectApp({ timeoutMs: 10000 });
assert.equal(link, "", "a reachable companion is asked without a link");
assert.equal(pairBody.origin, "https://papers.example");
assert.equal(pairBody.request, claimBody.request);
assert.equal(pairBody.challenge, createHash("sha256").update(claimBody.verifier).digest("hex"));
assert.ok(!JSON.stringify(pairBody).includes(claimBody.verifier));
assert.equal(local.status().state, "connected");

setup(() => response(403, {}));
await assert.rejects(local.connectApp(), /expired or was refused/);
assert.equal(attempts, 1);
assert.ok(!storage.has("librepaper-local-connections"));

setup(() => { local.configure({ origin: "https://other.example" }); return response(200, { token: "wrong-project", expires: 100000 }); });
await assert.rejects(local.connectApp(), /document changed/);
assert.ok(!storage.has("librepaper-local-connections"));

setup(() => response(202, {}));
await assert.rejects(local.connectApp({ timeoutMs: 1500 }), /did not connect/);
assert.ok(!storage.has("librepaper-local-connections"));

// Nothing installed, so the link starts nothing: the attempt gives up after
// the start grace with the explanation, not after the approval wait.
setup(() => { throw new TypeError("Failed to fetch"); });
await assert.rejects(local.connectApp(), (error) => error.name === "Unreachable"
  && /not running on this computer/.test(error.message) && /librepaper`/.test(error.message));
assert.equal(new URL(link).hostname, "connect", "the link is still tried first");
assert.ok(now >= 15000 && now < 30000, "gives up once the start grace has passed");
assert.ok(!storage.has("librepaper-local-pending"));

// A blocked Local Network Access permission fails with the same text as a
// stopped companion; the permission tells the two apart.
setup(() => { throw new TypeError("Failed to fetch"); });
permission = "denied";
await assert.rejects(local.connectApp(), (error) => error.name === "Refused" && /blocked this site/.test(error.message));

// Paired, then the companion stopped: the relaunch link fires, the probe
// keeps finding nothing, and the same explanation follows the grace.
setup((n) => (n === 1 ? response(202, {}) : response(200, { token: "stopped-token", expires: 1000000, instance: "restart" })));
companionUp = true;
await local.connectApp({ timeoutMs: 10000 });
local._testing.reset();
now = 0; link = ""; companionUp = false; healthUp = false;
inject(() => response(200, {}));
local.configure({ origin: "https://papers.example", project: "paper" });
await assert.rejects(local.connectApp(), /not running on this computer/);
assert.equal(new URL(link).hostname, "launch");

// A paired request that cannot reach the companion names the cause, not the
// browser's "Failed to fetch".
local._testing.inject({ fetch: async () => { throw new TypeError("Failed to fetch"); } });
await assert.rejects(local.agents(), (error) => /not running on this computer/.test(error.message) && !/Failed to fetch/.test(error.message));

local._testing.reset();
console.log("local-companion: cold launch, direct ask, verifier isolation, restart, already-connected, rejection, scope change and timeout, not running, blocked passed");

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

// Advertised address from the server: only used when page is loopback
{
  local._testing.reset();
  let changed = false;
  const storage = new Map();
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    location: () => ({ hostname: "127.0.0.1" }),
  });
  local.setAdvertisedAddress("http://127.0.0.1:9999/");
  assert.equal(local.address(), "http://127.0.0.1:9999/", "advertised address is used when page is loopback and no stored address");
}

{
  local._testing.reset();
  const storage = new Map();
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    location: () => ({ hostname: "example.com" }),
  });
  local.setAdvertisedAddress("http://127.0.0.1:9999/");
  assert.equal(local.address(), local.DEFAULT_ADDRESS, "advertised address is ignored when page host is not loopback");
}

{
  local._testing.reset();
  const storage = new Map();
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    location: () => ({ hostname: "127.0.0.1" }),
  });
  local.setAdvertisedAddress("https://127.0.0.1:9999/");
  assert.equal(local.address(), local.DEFAULT_ADDRESS, "advertised address is ignored when not http");
}

{
  local._testing.reset();
  const storage = new Map();
  storage.set("librepaper-local-address", "http://127.0.0.1:7777/");
  local._testing.inject({
    storage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: (k) => storage.delete(k) },
    location: () => ({ hostname: "127.0.0.1" }),
  });
  local.setAdvertisedAddress("http://127.0.0.1:9999/");
  assert.equal(local.address(), "http://127.0.0.1:7777/", "stored user address wins over advertised address");
}

local._testing.reset();
console.log("local-companion: setAdvertisedAddress uses advertised address when page is loopback and nothing stored, ignores when page host is not loopback or not http, and stored address wins over it");
