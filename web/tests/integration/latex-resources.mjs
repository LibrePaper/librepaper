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

async function sha256Hex(text) {
  const bytes = new TextEncoder().encode(text);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

const resources = await import("../../src/lib/latex/resources.js?latex-resources-check");

const release = { digest: "a".repeat(64), id: "2026-test" };
const other = { digest: "b".repeat(64), id: "2026-other" };

assert.equal(resources.namespace(release), `librepaper-latex-${"a".repeat(16)}`);
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
const second = await resources.fetchVerified(release, amsmathUrl, { sha256: amsmathSha, size: AMSMATH.length });
assert.equal(new TextDecoder().decode(await second.arrayBuffer()), AMSMATH);
assert.equal(network.calls, 1, "a cache hit does not refetch");

// Both pieces of manifest metadata are mandatory even when the URL is
// already cached: callers must never turn an unverified response into a
// trusted compiler input by omitting one check.
await assert.rejects(resources.fetchVerified(release, amsmathUrl, { sha256: amsmathSha }), /size/);
await assert.rejects(resources.fetchVerified(release, amsmathUrl, { size: AMSMATH.length }), /sha256/);
await assert.rejects(resources.fetchVerified(release, amsmathUrl, { sha256: "", size: AMSMATH.length }), /sha256/);
await assert.rejects(resources.fetchVerified(release, amsmathUrl, { sha256: amsmathSha, size: -1 }), /size/);

// --- sha256 mismatch on a fresh fetch: throws, and nothing is cached ------

const badUrl = "https://mirror.example/texlive/pdftex/26/bad.sty";
put(badUrl, "wrong bytes");
await assert.rejects(
  resources.fetchVerified(other, badUrl, { sha256: "0".repeat(64), size: 11 }),
  /sha256/,
);
assert.equal(
  await (await caches.open(resources.namespace(other))).match(badUrl),
  undefined,
  "a failed verification must not be cached as present",
);

// --- a corrupt cache hit is discarded and refetched -------------------------

const corruptUrl = "https://mirror.example/texlive/pdftex/26/corrupt.sty";
const CORRUPT_GOOD = "good bytes\n";
put(corruptUrl, CORRUPT_GOOD);
const corruptSha = await sha256Hex(CORRUPT_GOOD);
await resources.fetchVerified(release, corruptUrl, { sha256: corruptSha, size: CORRUPT_GOOD.length });
// Poison the cache entry directly (simulating storage corruption) without
// going through resources.js.
const store = await caches.open(resources.namespace(release));
await store.put(corruptUrl, new Response(new TextEncoder().encode("corrupted!!"), { headers: { "content-length": "11" } }));
network.calls = 0;
const healed = await resources.fetchVerified(release, corruptUrl, { sha256: corruptSha, size: CORRUPT_GOOD.length });
assert.equal(new TextDecoder().decode(await healed.arrayBuffer()), CORRUPT_GOOD);
assert.equal(network.calls, 1, "a corrupt hit triggers exactly one refetch, not a permanent failure");

// --- network error: thrown, never cached ------------------------------------

const missingUrl = "https://mirror.example/texlive/pdftex/26/missing.sty";
network.fail.add(missingUrl);
await assert.rejects(resources.fetchVerified(release, missingUrl, { sha256: "0".repeat(64), size: 0 }), /simulated network failure/);
assert.equal(await (await caches.open(resources.namespace(release))).match(missingUrl), undefined);

// A 404 also throws and caches nothing.
const notFoundUrl = "https://mirror.example/texlive/pdftex/26/nothere.sty";
await assert.rejects(resources.fetchVerified(release, notFoundUrl, { sha256: "0".repeat(64), size: 0 }), /404/);

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

// --- size() sums bytes across librepaper-latex-* caches only
// -------------------

const before = await resources.size();
assert.ok(before > 0, "size() reflects what has already been cached above");

// A cache under a different (non-latex) prefix must not be counted.
const unrelated = await caches.open("librepaper");
await unrelated.put("https://example.com/doc.pdf", new Response(new Uint8Array(1000), { headers: { "content-length": "1000" } }));
const afterUnrelated = await resources.size();
assert.equal(afterUnrelated, before, "size() ignores caches outside the librepaper-latex- prefix");

// --- clear() deletes every librepaper-latex-* cache and nothing else
// ----------

await resources.clear();
assert.equal(await resources.size(), 0);
const remainingNames = await caches.keys();
assert.ok(remainingNames.includes("librepaper"), "clear() must never touch project/document storage");
assert.ok(!remainingNames.some((n) => n.startsWith("librepaper-latex-")), "every latex cache is gone");

// --- working sets -----------------------------------------------------------

const resolutions = [[26, "amsmath.sty", "tex/latex/amsmath/amsmath.sty"], [26, "nothere.sty", null]];
const tar = Uint8Array.of(1, 2, 3, 4, 5);
await resources.writeWorkingSet(release, "pdftex", { resolutions, files: tar });
const stored = await resources.readWorkingSet(release, "pdftex");
assert.deepEqual(stored.resolutions, resolutions);
assert.deepEqual([...stored.files], [...tar]);
assert.equal(await resources.readWorkingSet(release, "xetex"), null, "working sets are per kind");
assert.equal(await resources.readWorkingSet(other, "pdftex"), null, "working sets are per release");
assert.ok(await resources.size() > 0, "working sets count toward size()");

// An empty tar still round trips.
await resources.writeWorkingSet(release, "bibtex", { resolutions: [], files: new Uint8Array(0) });
const emptied = await resources.readWorkingSet(release, "bibtex");
assert.deepEqual(emptied.resolutions, []);
assert.equal(emptied.files.length, 0);

// A corrupted body fails verification: the entry is deleted and null returned.
const wsKey = "https://librepaper.invalid/working-set/pdftex";
const wsStore = await caches.open(resources.namespace(release));
const good = await wsStore.match(wsKey);
const damaged = new Uint8Array(await good.arrayBuffer());
damaged[damaged.length - 1] ^= 0xff;
await wsStore.put(wsKey, new Response(damaged, { headers: good.headers }));
assert.equal(await resources.readWorkingSet(release, "pdftex"), null);
assert.equal(await wsStore.match(wsKey), undefined, "a corrupted working set is deleted");

// A malformed body whose digest matches (length prefix past the end) is deleted too.
const malformed = Uint8Array.of(255, 255, 0, 0, 1);
await wsStore.put(wsKey, new Response(malformed, {
  headers: { "x-librepaper-sha256": await sha256Hex(malformed), "content-length": "5" },
}));
assert.equal(await resources.readWorkingSet(release, "pdftex"), null);
assert.equal(await wsStore.match(wsKey), undefined, "a malformed working set is deleted");

// --- evictOtherReleases ------------------------------------------------------

await caches.open(resources.namespace(other));
await caches.open("librepaper-bundles");
await resources.evictOtherReleases(release);
const afterEvict = await caches.keys();
assert.ok(afterEvict.includes(resources.namespace(release)), "the current release is kept");
assert.ok(!afterEvict.includes(resources.namespace(other)), "other releases are deleted");
assert.ok(!afterEvict.includes("librepaper-bundles"), "the legacy engine cache is deleted");
assert.ok(afterEvict.includes("librepaper"), "unrelated caches are untouched");

// Memoised: a second call in the same page does not delete again.
await caches.open(resources.namespace(other));
await resources.evictOtherReleases(release);
assert.ok((await caches.keys()).includes(resources.namespace(other)), "eviction runs once per namespace");

// --- clear() also removes working sets ----------------------------------------

await resources.writeWorkingSet(release, "pdftex", { resolutions, files: tar });
await resources.clear();
assert.equal(await resources.readWorkingSet(release, "pdftex"), null);
assert.equal(await resources.size(), 0);

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
