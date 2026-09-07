// resources.js against fake `caches`/`fetch`/`crypto.subtle`: Node 24 has a
// real `crypto.subtle` and `CompressionStream`, but no Cache Storage and no
// network, so those two are faked here. The fakes are deliberately dumb (a
// Map of Map, a Map of bytes) so a bug in resources.js shows up as a wrong
// assertion here rather than being hidden by a clever fake reimplementing
// the same logic.
import assert from "node:assert/strict";

const backingStores = new Map(); // name -> Map(url -> Response)
class FakeCache {
  constructor(name) {
    this.name = name;
    this.map = backingStores.get(name) ?? new Map();
    backingStores.set(name, this.map);
  }
  async match(url) {
    const key = typeof url === "string" ? url : url.url;
    return this.map.get(key)?.clone() ?? undefined;
  }
  async put(url, response) {
    const key = typeof url === "string" ? url : url.url;
    this.map.set(key, response);
  }
  async delete(url) {
    const key = typeof url === "string" ? url : url.url;
    return this.map.delete(key);
  }
  async keys() {
    return [...this.map.keys()];
  }
}
globalThis.caches = {
  async open(name) {
    return new FakeCache(name);
  },
  async keys() {
    return [...backingStores.keys()];
  },
  async delete(name) {
    return backingStores.delete(name);
  },
};

// The network: a small fixed catalog of byte payloads keyed by URL, served
// through `fetch`. `network.calls` counts requests so the check can assert a
// cache hit avoids a second fetch.
const network = { calls: 0, payloads: new Map(), fail: new Set() };
function put(url, text) {
  network.payloads.set(url, new TextEncoder().encode(text));
}
globalThis.fetch = async (url) => {
  network.calls++;
  const key = typeof url === "string" ? url : url.toString();
  if (network.fail.has(key)) throw new Error("simulated network failure");
  const bytes = network.payloads.get(key);
  if (!bytes) return new Response(null, { status: 404 });
  return new Response(bytes, { status: 200, headers: { "content-length": String(bytes.length) } });
};

globalThis.localStorage = (() => {
  const store = new Map();
  return {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: (k) => store.delete(k),
  };
})();

async function sha256Hex(text) {
  const bytes = new TextEncoder().encode(text);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

const resources = await import("../src/lib/latex/resources.js?latex-resources-check");

const release = { digest: "a".repeat(64), id: "2026-test" };
const other = { digest: "b".repeat(64), id: "2026-other" };

assert.equal(resources.namespace(release), `komodoc-latex-${"a".repeat(16)}`);
assert.notEqual(resources.namespace(release), resources.namespace(other));

// --- fetchVerified: fresh fetch, verified, cached --------------------------

const AMSMATH = "\\amsmath contents\n";
const amsmathUrl = "https://mirror.example/texlive/pdftex/26/amsmath.sty";
put(amsmathUrl, AMSMATH);
const amsmathSha = await sha256Hex(AMSMATH);

network.calls = 0;
const first = await resources.fetchVerified(release, amsmathUrl, { sha256: amsmathSha, size: AMSMATH.length });
assert.equal(new TextDecoder().decode(await first.arrayBuffer()), AMSMATH);
assert.equal(network.calls, 1, "first fetchVerified call hits the network");

// A second call must be served from the cache, not the network.
const second = await resources.fetchVerified(release, amsmathUrl, { sha256: amsmathSha });
assert.equal(new TextDecoder().decode(await second.arrayBuffer()), AMSMATH);
assert.equal(network.calls, 1, "a cache hit does not refetch");

// --- sha256 mismatch on a fresh fetch: throws, and nothing is cached ------

const badUrl = "https://mirror.example/texlive/pdftex/26/bad.sty";
put(badUrl, "wrong bytes");
await assert.rejects(
  resources.fetchVerified(other, badUrl, { sha256: "0".repeat(64) }),
  /sha256/,
);
const readAfterBadFetch = await resources.readiness(other, [{ key: "pdftex/26/bad.sty", url: badUrl }]);
assert.equal(readAfterBadFetch.ready, false, "a failed verification must not be cached as present");

// --- a corrupt cache hit is discarded and refetched -------------------------

const corruptUrl = "https://mirror.example/texlive/pdftex/26/corrupt.sty";
const CORRUPT_GOOD = "good bytes\n";
put(corruptUrl, CORRUPT_GOOD);
const corruptSha = await sha256Hex(CORRUPT_GOOD);
await resources.fetchVerified(release, corruptUrl, { sha256: corruptSha });
// Poison the cache entry directly (simulating storage corruption) without
// going through resources.js.
const store = await caches.open(resources.namespace(release));
await store.put(corruptUrl, new Response(new TextEncoder().encode("corrupted!!"), { headers: { "content-length": "11" } }));
network.calls = 0;
const healed = await resources.fetchVerified(release, corruptUrl, { sha256: corruptSha });
assert.equal(new TextDecoder().decode(await healed.arrayBuffer()), CORRUPT_GOOD);
assert.equal(network.calls, 1, "a corrupt hit triggers exactly one refetch, not a permanent failure");

// --- network error: thrown, never cached ------------------------------------

const missingUrl = "https://mirror.example/texlive/pdftex/26/missing.sty";
network.fail.add(missingUrl);
await assert.rejects(resources.fetchVerified(release, missingUrl, {}), /simulated network failure/);
const readMissing = await resources.readiness(release, [{ key: "pdftex/26/missing.sty", url: missingUrl }]);
assert.equal(readMissing.ready, false);

// A 404 also throws and caches nothing.
const notFoundUrl = "https://mirror.example/texlive/pdftex/26/nothere.sty";
await assert.rejects(resources.fetchVerified(release, notFoundUrl, {}), /404/);

// --- prefetch: parallel, verified, progress reported ------------------------

const entries = [];
for (let i = 0; i < 9; i++) {
  const url = `https://mirror.example/texlive/pdftex/26/pkg${i}.sty`;
  const text = `package ${i}\n`;
  put(url, text);
  entries.push({ url, sha256: await sha256Hex(text), size: text.length });
}
const progress = [];
network.calls = 0;
const results = await resources.prefetch(release, entries, (p) => progress.push({ ...p }));
assert.equal(results.length, 9);
assert.equal(network.calls, 9);
assert.equal(progress.at(-1).done, 9);
assert.equal(progress.at(-1).total, 9);
assert.ok(progress.length >= 2, "progress reported incrementally, not just once at the end");

// --- size() sums bytes across komodoc-latex-* caches only -------------------

const before = await resources.size();
assert.ok(before > 0, "size() reflects what has already been cached above");

// A cache under a different (non-latex) prefix must not be counted.
const unrelated = await caches.open("komodoc");
await unrelated.put("https://example.com/doc.pdf", new Response(new Uint8Array(1000), { headers: { "content-length": "1000" } }));
const afterUnrelated = await resources.size();
assert.equal(afterUnrelated, before, "size() ignores caches outside the komodoc-latex- prefix");

// --- clear() deletes every komodoc-latex-* cache and nothing else ----------

await resources.clear();
assert.equal(await resources.size(), 0);
const remainingNames = await caches.keys();
assert.ok(remainingNames.includes("komodoc"), "clear() must never touch project/document storage");
assert.ok(!remainingNames.some((n) => n.startsWith("komodoc-latex-")), "every latex cache is gone");

// --- readiness() after clear: everything missing again ----------------------

const readAfterClear = await resources.readiness(release, [{ key: "pdftex/26/amsmath.sty", url: amsmathUrl }]);
assert.equal(readAfterClear.ready, false);
assert.deepEqual(readAfterClear.missing, ["pdftex/26/amsmath.sty"]);

// --- remember(): bounded to 2000 keys per release, in localStorage ---------

for (let i = 0; i < 2005; i++) resources.remember(release, `pdftex/26/gen${i}.sty`);
const kept = resources.remembered(release);
assert.equal(kept.length, 2000, "remember() is bounded to 2000 keys per release");
assert.ok(!kept.includes("pdftex/26/gen0.sty"), "oldest entries are dropped first");
assert.ok(kept.includes("pdftex/26/gen2004.sty"), "the newest entry survives");

// A different release's remembered keys are tracked separately.
resources.remember(other, "pdftex/26/onlyother.sty");
assert.equal(resources.remembered(other).length, 1);
assert.equal(resources.remembered(release).length, 2000);

// --- persist(): tolerates refusal, called once ------------------------------

let persistCalls = 0;
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: { storage: { persist: async () => { persistCalls++; return false; } } },
});
assert.equal(await resources.persist(), false);
assert.equal(await resources.persist(), false);
assert.equal(persistCalls, 1, "persist() asks navigator.storage.persist at most once");

console.log("latex-resources: ok");
