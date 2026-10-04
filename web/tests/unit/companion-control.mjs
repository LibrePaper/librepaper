import assert from "node:assert/strict";
import { createControlClient } from "../../src/lib/companion/control.js";

const address = "http://127.0.0.1:8763/";
const token = "0123456789abcdefghijklmnopqrstuvwxyz_ABCDEF";
const instance = "0123456789abcdef";

function harness(hash = "") {
  const store = new Map();
  const requests = [];
  const replaced = [];
  let currentFetch = async (url, init) => {
    requests.push({ url, init });
    return { ok: true, status: 200, headers: { get: () => "application/json" }, json: async () => ({ ok: true }) };
  };
  const location = { origin: "https://app.example.test", href: `https://app.example.test/${hash ? `#${hash}` : ""}`, hash: hash ? `#${hash}` : "" };
  const deps = {
    location: () => location,
    storage: () => ({
      getItem: (key) => store.get(key) ?? null,
      setItem: (key, value) => store.set(key, String(value)),
      removeItem: (key) => store.delete(key),
    }),
    replaceUrl: (value) => {
      replaced.push(value);
      const next = new URL(value, location.origin);
      location.href = next.href;
      location.hash = next.hash;
    },
    fetch: (...args) => currentFetch(...args),
    launchLink: (url) => requests.push({ launch: url }),
  };
  return { client: createControlClient(deps), location, store, requests, replaced, setFetch: (fn) => { currentFetch = fn; } };
}

// Intake removes all secret fields before rendering while retaining unrelated
// document fragments used by the reader.
{
  const h = harness(`k=reader-key&settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=${instance}`);
  const updates = [];
  const unsubscribe = h.client.subscribe((value) => updates.push(value));
  assert.equal(h.client.intake(), true);
  assert.equal(h.location.hash, "#k=reader-key");
  assert.equal(h.replaced.length, 1);
  assert.equal(h.replaced[0], "/#k=reader-key");
  assert.equal(h.client.available(), true);
  assert.equal(h.client.scope(), `${address}|${instance}`);
  assert.equal([...h.store.values()].some((value) => String(value).includes(token)), true);
  assert.deepEqual(updates, [
    { available: false, scope: "" },
    { available: true, scope: `${address}|${instance}` },
  ]);
  unsubscribe();
  assert.equal(h.client.intake(), false);
}

// Invalid and partial handoffs still lose their URL secrets. A non-loopback
// address never produces an available client or a request carrying a bearer.
{
  const h = harness(`settings=local&companion_address=${encodeURIComponent("https://attacker.example/")}&companion_control=${token}&companion_instance=${instance}&keep=yes`);
  assert.equal(h.client.intake(), true);
  assert.equal(h.location.hash, "#keep=yes");
  assert.equal(h.client.available(), false);
  await assert.rejects(h.client.request("/state"), { name: "Unavailable" });
  assert.equal(h.requests.length, 0);
}

// Requests are pinned to the loopback API, omit ambient credentials, and send
// the control key only in the authorization header with no-store semantics.
{
  const h = harness(`settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=${instance}`);
  h.client.intake();
  assert.deepEqual(await h.client.request("/state"), { ok: true });
  const [{ url, init }] = h.requests;
  assert.equal(url, "http://127.0.0.1:8763/companion/api/state");
  assert.equal(init.credentials, "omit");
  assert.equal(init.cache, "no-store");
  assert.equal(init.targetAddressSpace, "loopback");
  assert.equal(init.headers.get("Authorization"), `Bearer ${token}`);
  assert.equal(init.headers.has("Cookie"), false);
}

// An older in-flight rejection cannot clear a newer credential intake.
{
  const h = harness(`settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=${instance}`);
  h.client.intake();
  let finish;
  h.setFetch(() => new Promise((resolve) => { finish = resolve; }));
  const pending = h.client.request("/state");
  // Replace the session using a new instance while the first request is live.
  h.location.href = `https://app.example.test/#settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=abcdef0123456789`;
  h.location.hash = new URL(h.location.href).hash;
  h.client.intake();
  assert.equal(h.client.scope(), `${address}|abcdef0123456789`);
  finish({ ok: false, status: 401, headers: { get: () => "application/json" }, json: async () => ({ error: "expired" }) });
  await assert.rejects(pending, { name: "Unauthorized" });
  assert.equal(h.client.available(), true);
  assert.equal(h.client.scope(), `${address}|abcdef0123456789`);
}

// Even a forged sessionStorage descriptor cannot cause a bearer to leave for
// a public host; addresses are validated again when credentials are read.
{
  const h = harness();
  const app = encodeURIComponent("https://app.example.test");
  const publicAddress = "https://attacker.example/";
  const key = `librepaper:companion-control:v1:${app}:${encodeURIComponent(publicAddress)}:${instance}`;
  h.store.set(key, token);
  h.store.set(`librepaper:companion-control:v1:active:https://app.example.test`, JSON.stringify({ address: publicAddress, instance }));
  assert.equal(h.client.available(), false);
  await assert.rejects(h.client.request("/state"), { name: "Unavailable" });
  assert.equal(h.requests.length, 0);
}

{
  const h = harness();
  let opened = 0;
  h.client.onSettingsRequested(() => opened++);
  h.client.showSettings();
  assert.equal(opened, 1);
  assert.deepEqual(h.requests, []);
}

// A user-initiated trusted session is stored for the active app and emits no
// navigation. The response address must remain the exact loopback target.
{
  const h = harness();
  h.setFetch(async (url, init) => {
    h.requests.push({ url, init });
    return { ok: true, status: 200, json: async () => ({ address, token, instance }) };
  });
  const connected = await h.client.connect(address);
  assert.deepEqual(connected, { address, instance });
  assert.equal(h.client.available(), true);
  assert.equal(h.client.scope(), `${address}|${instance}`);
  assert.equal(h.requests[0].url, `${address}companion/api/session`);
  assert.equal(h.requests[0].init.method, "POST");
  assert.equal(h.requests[0].init.cache, "no-store");
  assert.equal(h.requests.some((item) => item.launch), false);
}

// Session bootstrap rejects credentials for any other host or malformed
// identity, even when the response came from a loopback fetch.
for (const session of [
  { address: "http://localhost:8763/", token, instance },
  { address, token: "bad", instance },
  { address, token, instance: "bad" },
]) {
  const h = harness();
  h.setFetch(async () => ({ ok: true, status: 200, json: async () => session }));
  await assert.rejects(h.client.connect(address), { name: "InvalidSession" });
  assert.equal(h.client.available(), false);
}

// A newer explicit address choice wins, and an intake during a pending
// request cancels that reply before it can replace the active session.
{
  const h = harness();
  const deferred = [];
  h.setFetch(() => new Promise((resolve) => deferred.push(resolve)));
  const first = h.client.connect(address);
  const secondAddress = "http://127.0.0.1:8764/";
  const second = h.client.connect(secondAddress);
  deferred[1]({ ok: true, status: 200, json: async () => ({ address: secondAddress, token, instance }) });
  await second;
  deferred[0]({ ok: true, status: 200, json: async () => ({ address, token, instance }) });
  await assert.rejects(first, { name: "Canceled" });
  assert.equal(h.client.scope(), `${secondAddress}|${instance}`);
}

{
  const h = harness();
  let finish;
  h.setFetch(() => new Promise((resolve) => { finish = resolve; }));
  const pending = h.client.connect(address);
  h.location.href = "https://app.example.test/#settings=local&keep=yes";
  h.location.hash = new URL(h.location.href).hash;
  h.client.intake();
  finish({ ok: true, status: 200, json: async () => ({ address, token, instance }) });
  await assert.rejects(pending, { name: "Canceled" });
  assert.equal(h.client.available(), false);
}

// If the browser denies sessionStorage, the control token remains available
// only in this page's memory and can still open Settings for this handoff.
{
  const location = {
    origin: "https://app.example.test",
    href: `https://app.example.test/#settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=${instance}`,
    get hash() { return new URL(this.href).hash; },
  };
  const client = createControlClient({
    location: () => location,
    storage: () => { throw new Error("storage denied"); },
    replaceUrl: (url) => { location.href = new URL(url, location.origin).href; },
    fetch: async () => ({ ok: true, status: 204 }),
  });
  assert.equal(client.intake(), true);
  assert.equal(client.available(), true);
  assert.equal(client.scope(), `${address}|${instance}`);
  await client.request("/state");
}

// A 401 revokes the in-memory credential even when sessionStorage access
// refuses removal, so a later view cannot revive and reuse the rejected bearer.
{
  const store = new Map();
  let denyRemovals = false;
  const location = {
    origin: "https://app.example.test",
    href: `https://app.example.test/#settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=${instance}`,
    get hash() { return new URL(this.href).hash; },
  };
  let fetches = 0;
  const client = createControlClient({
    location: () => location,
    storage: () => ({
      getItem: (key) => store.get(key) ?? null,
      setItem: (key, value) => store.set(key, String(value)),
      removeItem: (key) => {
        if (denyRemovals) throw new Error("storage removal denied");
        store.delete(key);
      },
    }),
    replaceUrl: (url) => { location.href = new URL(url, location.origin).href; },
    fetch: async () => {
      fetches++;
      return { ok: false, status: 401, json: async () => ({ error: "invalid control token" }) };
    },
  });
  client.intake();
  denyRemovals = true;
  assert.equal(client.available(), true);
  await assert.rejects(client.request("/state"), { name: "Unauthorized" });
  assert.equal(client.available(), false);
  await assert.rejects(client.request("/state"), { name: "Unavailable" });
  assert.equal(fetches, 1);
}

// A quota failure while persisting the bearer falls back to the current page's
// memory handoff and still keeps the URL clean.
{
  const store = new Map();
  const location = {
    origin: "https://app.example.test",
    href: `https://app.example.test/#settings=local&companion_address=${encodeURIComponent(address)}&companion_control=${token}&companion_instance=${instance}`,
    get hash() { return new URL(this.href).hash; },
  };
  const client = createControlClient({
    location: () => location,
    storage: () => ({
      getItem: (key) => store.get(key) ?? null,
      setItem: () => { throw new Error("quota exceeded"); },
      removeItem: (key) => store.delete(key),
    }),
    replaceUrl: (url) => { location.href = new URL(url, location.origin).href; },
    fetch: async () => ({ ok: true, status: 204 }),
  });
  assert.equal(client.intake(), true);
  assert.equal(new URL(location.href).hash, "");
  assert.equal(client.available(), true);
  await client.request("/state");
}
