// The offline worker's fetch listener: which requests it answers and which it
// lets go by. The worker is loaded as the browser loads it, as a script with
// `self`, `caches` and `fetch` in scope, so this reads the shipped file.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../../public/offline-sw.js", import.meta.url), "utf8");

const listeners = {};
const fetched = [];
const self = {
  location: { origin: "https://app.example" },
  addEventListener: (type, listener) => { listeners[type] = listener; },
  skipWaiting() {},
  clients: { claim: async () => {} },
};
const fetchStub = (request) => {
  fetched.push(request);
  return Promise.resolve({ ok: true, status: 200, clone() { return this; } });
};
const caches = {
  open: async () => ({ add: async () => {}, put: async () => {} }),
  keys: async () => [],
  match: async () => undefined,
  delete: async () => true,
};
vm.runInNewContext(source, { self, caches, fetch: fetchStub, URL });

/** Sends one request through the worker and reports what it answered with, if anything. */
function dispatch(url, { method = "GET", mode = "cors", destination = "" } = {}) {
  const request = { url, method, mode, destination };
  let answer;
  listeners.fetch({ request, respondWith: (value) => { answer = value; }, waitUntil() {} });
  return { request, answer };
}

// A DOI lookup is made by the worker itself, with the page's own request, so
// its headers survive doi.org's redirect under WebKit.
const lookup = dispatch("https://doi.org/10.1086/734279");
assert.ok(lookup.answer, "a GET to doi.org is answered by the worker");
assert.equal(fetched.length, 1);
assert.equal(fetched[0], lookup.request, "the worker forwards the request it was given");
await lookup.answer;

// Nothing else off the site is the worker's business.
for (const url of [
  "https://api.crossref.org/v1/works/x/transform",
  "https://example.org/doi.org",
  "https://doi.org.example.net/10.1/x",
]) {
  assert.equal(dispatch(url).answer, undefined, url);
}
assert.equal(dispatch("https://doi.org/10.1086/734279", { method: "POST" }).answer, undefined, "only GET is relayed");

// On the site itself the API and the socket go by, and the shell is answered.
assert.equal(dispatch("https://app.example/api/documents").answer, undefined);
assert.equal(dispatch("https://app.example/ws/room").answer, undefined);
const shell = dispatch("https://app.example/assets/app.js", { destination: "script" });
assert.ok(shell.answer, "a shell resource is answered by the worker");
await shell.answer;

console.log("offline worker: relays DOI lookups, leaves other sites, the API and the socket alone");
