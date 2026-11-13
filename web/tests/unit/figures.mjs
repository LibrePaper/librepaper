import assert from "node:assert/strict";
import {
  __resetAssetTestState,
  __setAssetTestHooks,
  gather,
  keep,
  release,
} from "../../src/lib/figures.js";
import { ASSET_DATABASE } from "../../src/lib/local-data.js";
import { markSignedOut } from "../../src/lib/signed-out.js";

const hex = (bytes) => [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
const digestOf = async (bytes) => hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));
const tick = () => new Promise((resolve) => setTimeout(resolve, 15));
const linkUrl = (sha) => `/api/documents/doc/assets/${sha}/link`;
const bytesUrl = (sha) => `/api/documents/doc/assets/${sha}`;
const isLink = (url) => url.startsWith("/api/") && url.endsWith("/link");
const isBucket = (url) => url.startsWith("https://bucket.example/");

function reply(status, { bytes, json, headers = {} } = {}) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers(headers),
    arrayBuffer: async () => (bytes ?? new Uint8Array()).slice().buffer,
    json: async () => json,
  };
}

const noLink = () => reply(200, { json: { url: null } });

// A reply whose body read fails the first time, as a dropped connection does.
function flakyReply(status, options) {
  const good = reply(status, options);
  let failed = false;
  const flaky = (read) => async () => {
    if (!failed) {
      failed = true;
      throw new TypeError("network error");
    }
    return read();
  };
  return {
    ...good,
    arrayBuffer: flaky(good.arrayBuffer),
    json: flaky(good.json),
  };
}

// An in-memory IndexedDB: enough of the API for the store, with requests
// answered on the next turn. Stores are plain Maps keyed by digest.
function fakeIndexedDB() {
  const databases = new Map(); // name -> Map<store name, Map<key, value>>
  const storeOf = (stores, name) => {
    const records = stores.get(name);
    const answer = (run) => {
      const request = {};
      let result;
      let error = null;
      try {
        result = run();
      } catch (thrown) {
        error = thrown;
      }
      setTimeout(() => {
        if (error) {
          request.error = error;
          request.onerror?.();
        } else {
          request.result = result;
          request.onsuccess?.();
        }
      });
      return request;
    };
    return {
      get: (key) => answer(() => structuredClone(records.get(key))),
      put: (value, key) => answer(() => records.set(key, structuredClone(value)) && key),
      delete: (key) => answer(() => records.delete(key)),
      createIndex() {},
      index() {
        return {
          openCursor() {
            const rows = [...records].sort(([ak, av], [bk, bv]) => av.used - bv.used || (ak < bk ? -1 : 1));
            const request = {};
            let at = 0;
            const step = () => {
              const i = at;
              request.result = i < rows.length
                ? { primaryKey: rows[i][0], value: rows[i][1], continue() { at = i + 1; setTimeout(step); } }
                : null;
              request.onsuccess?.();
            };
            setTimeout(step);
            return request;
          },
        };
      },
    };
  };
  const api = {
    databases,
    opens: 0,
    open(name) {
      api.opens++;
      const request = {};
      setTimeout(() => {
        const fresh = !databases.has(name);
        if (fresh) databases.set(name, new Map());
        const stores = databases.get(name);
        request.result = {
          objectStoreNames: { contains: (n) => stores.has(n) },
          createObjectStore(n) {
            stores.set(n, new Map());
            return storeOf(stores, n);
          },
          // Accepts one store name or a list; the store is named per objectStore call.
          transaction: () => ({ objectStore: (s) => storeOf(stores, s) }),
          close() {},
        };
        if (fresh) request.onupgradeneeded?.();
        request.onsuccess?.();
      });
      return request;
    },
  };
  return api;
}

const sleeps = [];
// Runs `run` with `route` answering every fetch, and restores everything after.
async function scenario(route, run, hooks = {}) {
  const realFetch = globalThis.fetch;
  const calls = [];
  sleeps.length = 0;
  globalThis.fetch = async (url, init) => {
    calls.push({ url: String(url), init });
    return route(String(url), init);
  };
  __resetAssetTestState();
  __setAssetTestHooks({ sleep: async (ms) => { sleeps.push(ms); }, random: () => 0, ...hooks });
  try {
    await run(calls);
  } finally {
    globalThis.fetch = realFetch;
    __setAssetTestHooks({ sleep: undefined, random: undefined, indexedDB: undefined, capBytes: undefined });
    __resetAssetTestState();
  }
}

__resetAssetTestState();

// Test that gather() creates data URLs for images and blob URLs for PDFs.
{
  const pngBytes = Uint8Array.of(0x89, 0x50, 0x4e, 0x47);
  const svgBytes = new TextEncoder().encode("<svg></svg>");
  const pdfBytes = Uint8Array.of(0x25, 0x50, 0x44, 0x46);

  // Stub fetch to return the bytes for each figure.
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (url.endsWith("/link")) return noLink();
    if (url.endsWith("/assets/a")) return reply(200, { bytes: pngBytes });
    if (url.endsWith("/assets/b")) return reply(200, { bytes: svgBytes });
    if (url.endsWith("/assets/c")) return reply(200, { bytes: pdfBytes });
    throw new Error(`Unexpected fetch: ${url}`);
  };

  // Stub URL.createObjectURL for the PDF test case.
  const originalCreateObjectURL = URL.createObjectURL;
  URL.createObjectURL = () => "blob:test-blob-url";

  try {
    const result = await gather("doc", {
      "fig.png": "a",
      "fig.svg": "b",
      "fig.pdf": "c",
    });

    // PNG should be a data URL starting with data:image/png;base64,
    const pngUrl = result.urls["fig.png"];
    assert.ok(pngUrl.startsWith("data:image/png;base64,"), "PNG URL starts with data:image/png;base64,");
    assert.ok(pngUrl.includes("#librepaper-asset=a"), "PNG URL includes fragment with asset digest");

    // SVG should be a data URL starting with data:image/svg+xml;base64,
    const svgUrl = result.urls["fig.svg"];
    assert.ok(svgUrl.startsWith("data:image/svg+xml;base64,"), "SVG URL starts with data:image/svg+xml;base64,");
    assert.ok(svgUrl.includes("#librepaper-asset=b"), "SVG URL includes fragment with asset digest");

    // PDF should be a blob URL
    const pdfUrl = result.urls["fig.pdf"];
    assert.ok(pdfUrl.startsWith("blob:"), "PDF URL starts with blob:");
    assert.ok(pdfUrl.includes("#librepaper-asset=c"), "PDF URL includes fragment with asset digest");

    // All assets should be present as bytes
    assert.deepEqual([...result.assets["fig.png"]], [...pngBytes]);
    assert.deepEqual([...result.assets["fig.svg"]], [...svgBytes]);
    assert.deepEqual([...result.assets["fig.pdf"]], [...pdfBytes]);

    // No missing figures
    assert.deepEqual(result.missing, []);

    // release() should be called to clean up blob URLs
    release();
  } finally {
    URL.createObjectURL = originalCreateObjectURL;
    globalThis.fetch = originalFetch;
  }
}

// A link is asked for, and the bucket is fetched with no credentials and no
// custom headers. The byte route is not used.
{
  const body = Uint8Array.of(1, 2, 3, 4);
  const sha = await digestOf(body);
  await scenario((url) => {
    if (isLink(url)) return reply(200, { json: { url: "https://bucket.example/o?sig=1" } });
    if (isBucket(url)) return reply(200, { bytes: body });
    throw new Error(`unexpected ${url}`);
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha }, { Authorization: "x" });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    assert.deepEqual(calls.map((call) => call.url), [linkUrl(sha), "https://bucket.example/o?sig=1"]);
    assert.equal(calls[1].init.credentials, "omit");
    assert.equal(calls[1].init.headers, undefined);
  });
}

// A null link sends the tab to the byte route, and a later asset in the same
// tab does not ask for a link at all.
{
  const a = Uint8Array.of(1);
  const b = Uint8Array.of(2);
  const shaA = await digestOf(a);
  const shaB = await digestOf(b);
  const served = { [shaA]: a, [shaB]: b };
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: served[url.split("/").pop()] });
  }, async (calls) => {
    const first = await gather("doc", { "a.png": shaA });
    const second = await gather("doc", { "b.png": shaB });
    assert.deepEqual([...first.assets["a.png"]], [...a]);
    assert.deepEqual([...second.assets["b.png"]], [...b]);
    assert.equal(calls.filter((call) => isLink(call.url)).length, 1);
    assert.deepEqual(calls.slice(1).map((call) => call.url), [bytesUrl(shaA), bytesUrl(shaB)]);
  });
}

// A link route that answers 404 sends the asset to the byte route, does not
// fail it, and a server without the route is not asked again in this tab.
{
  const a = Uint8Array.of(21);
  const b = Uint8Array.of(22);
  const shaA = await digestOf(a);
  const shaB = await digestOf(b);
  const served = { [shaA]: a, [shaB]: b };
  await scenario((url) => {
    if (isLink(url)) return reply(404);
    return reply(200, { bytes: served[url.split("/").pop()] });
  }, async (calls) => {
    const first = await gather("doc", { "a.png": shaA });
    const second = await gather("doc", { "b.png": shaB });
    assert.deepEqual([...first.assets["a.png"]], [...a]);
    assert.deepEqual([...second.assets["b.png"]], [...b]);
    assert.deepEqual(first.missing, []);
    assert.deepEqual(second.missing, []);
    assert.equal(calls.filter((call) => isLink(call.url)).length, 1, "the second asset skips the link request");
    assert.deepEqual(calls.slice(1).map((call) => call.url), [bytesUrl(shaA), bytesUrl(shaB)]);
  });
}

// A bucket that fails as a network error falls back to the byte route, and
// the tab keeps using that route.
{
  const body = Uint8Array.of(5, 6);
  const sha = await digestOf(body);
  const other = Uint8Array.of(7);
  const shaOther = await digestOf(other);
  const served = { [sha]: body, [shaOther]: other };
  await scenario((url) => {
    if (isLink(url)) return reply(200, { json: { url: `https://bucket.example/object/${url.split("/")[5]}` } });
    if (isBucket(url)) throw new TypeError("Failed to fetch");
    return reply(200, { bytes: served[url.split("/").pop()] });
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    await gather("doc", { "other.png": shaOther });
    assert.equal(calls.filter((call) => isBucket(call.url)).length, 1);
    assert.equal(calls.filter((call) => isLink(call.url)).length, 1);
  });
}

// A bucket 403 asks for one new link and retries once; if that fails too the
// byte route serves the figure, and the tab still asks for links afterwards.
{
  const body = Uint8Array.of(8);
  const sha = await digestOf(body);
  const other = Uint8Array.of(9);
  const shaOther = await digestOf(other);
  const served = { [sha]: body, [shaOther]: other };
  let links = 0;
  await scenario((url) => {
    if (isLink(url)) {
      links++;
      return reply(200, { json: { url: `https://bucket.example/object${links}` } });
    }
    if (isBucket(url)) return reply(403);
    return reply(200, { bytes: served[url.split("/").pop()] });
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    assert.equal(calls.filter((call) => isLink(call.url)).length, 2);
    assert.equal(calls.filter((call) => isBucket(call.url)).length, 2);
    await gather("doc", { "other.png": shaOther });
    // Each asset asks for a link and reissues once; a 403 never switches the tab.
    assert.equal(calls.filter((call) => isLink(call.url)).length, 4, "a 403 does not switch the tab to the byte route");
    assert.equal(calls.filter((call) => isBucket(call.url)).length, 4);
  });
}

// A 429 for transfer permits is retried after Retry-After, plus jitter.
{
  const body = Uint8Array.of(10);
  const sha = await digestOf(body);
  let links = 0;
  await scenario((url) => {
    if (isLink(url)) {
      links++;
      if (links === 1) return reply(429, { json: { reason: "transfer_concurrency" }, headers: { "Retry-After": "2" } });
      return noLink();
    }
    return reply(200, { bytes: body });
  }, async () => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual(result.missing, []);
    assert.deepEqual(sleeps, [2000]);
    assert.equal(links, 2);
  });
}

// Five transfer refusals in a row are given up on, and the figure is missing.
{
  const sha = await digestOf(Uint8Array.of(11));
  await scenario((url) => {
    if (isLink(url)) return reply(429, { json: { reason: "transfer_concurrency" }, headers: { "Retry-After": "1" } });
    throw new Error(`unexpected ${url}`);
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual(result.missing, ["fig.png"]);
    assert.equal(calls.length, 6, "one request and five retries");
    assert.deepEqual(sleeps, [1000, 1000, 1000, 1000, 1000]);
  });
}

// A request_budget refusal is not retried.
{
  const sha = await digestOf(Uint8Array.of(12));
  await scenario((url) => {
    if (isLink(url)) return reply(429, { json: { reason: "request_budget" }, headers: { "Retry-After": "60" } });
    throw new Error(`unexpected ${url}`);
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual(result.missing, ["fig.png"]);
    assert.equal(calls.length, 1);
    assert.deepEqual(sleeps, []);
  });
}

// A digest mismatch is not retried.
{
  const sha = await digestOf(Uint8Array.of(13));
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: Uint8Array.of(99) });
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual(result.missing, ["fig.png"]);
    assert.equal(calls.filter((call) => !isLink(call.url)).length, 1);
    assert.deepEqual(sleeps, []);
  });
}

// A stored entry that fails its digest is deleted, and the figure is fetched
// and stored again.
{
  const body = Uint8Array.of(14, 15);
  const sha = await digestOf(body);
  const fake = fakeIndexedDB();
  fake.databases.set(ASSET_DATABASE, new Map([
    ["bytes", new Map([[sha, Uint8Array.of(0)]])],
    ["meta", new Map([[sha, { size: 1, used: 1 }]])],
  ]));
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: body });
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    assert.equal(calls.filter((call) => !isLink(call.url)).length, 1, "fetched again");
    await tick();
    const stored = fake.databases.get(ASSET_DATABASE).get("bytes").get(sha);
    assert.deepEqual([...stored], [...body]);
    assert.equal(fake.databases.get(ASSET_DATABASE).get("meta").get(sha).size, body.byteLength);
  }, { indexedDB: fake });
}

// The persistent store stays under its cap: the least recently used entries
// go first, and an asset larger than the cap is never stored.
{
  const make = (fill) => Uint8Array.of(...new Array(40).fill(fill));
  const first = make(1);
  const second = make(2);
  const third = make(3);
  const huge = new Uint8Array(200).fill(4);
  const all = [first, second, third, huge];
  const shas = await Promise.all(all.map(digestOf));
  const served = Object.fromEntries(shas.map((sha, i) => [sha, all[i]]));
  const fake = fakeIndexedDB();
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: served[url.split("/").pop()] });
  }, async () => {
    const bytesStore = () => fake.databases.get(ASSET_DATABASE).get("bytes");
    const metaStore = () => fake.databases.get(ASSET_DATABASE).get("meta");
    await gather("doc", { "1.png": shas[0] });
    await tick();
    await gather("doc", { "2.png": shas[1] });
    await tick();
    await gather("doc", { "3.png": shas[2] });
    await tick();
    const expected = [shas[1], shas[2]].sort();
    assert.deepEqual([...metaStore().keys()].sort(), expected, "the oldest entry was evicted from meta");
    assert.deepEqual([...bytesStore().keys()].sort(), expected, "and from bytes");
    await gather("doc", { "huge.png": shas[3] });
    await tick();
    assert.equal(bytesStore().has(shas[3]), false, "an asset over the cap is not stored");
  }, { indexedDB: fake, capBytes: 100 });
}

// With no IndexedDB at all the figures are still fetched.
{
  const body = Uint8Array.of(16);
  const sha = await digestOf(body);
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: body });
  }, async () => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    assert.deepEqual(result.missing, []);
  }, { indexedDB: null });
}

// A byte-route body that fails to read once is asked for again after one
// backoff, and the figure arrives.
{
  const body = Uint8Array.of(31, 32);
  const sha = await digestOf(body);
  let byteFetches = 0;
  await scenario((url) => {
    if (isLink(url)) return noLink();
    byteFetches++;
    return byteFetches === 1 ? flakyReply(200, { bytes: body }) : reply(200, { bytes: body });
  }, async (calls) => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    assert.deepEqual(result.missing, []);
    assert.deepEqual(sleeps, [500]);
    assert.equal(calls.filter((call) => !isLink(call.url)).length, 2);
  });
}

// A link body that fails to read once is asked for again, and the figure still
// arrives by the byte route.
{
  const body = Uint8Array.of(33);
  const sha = await digestOf(body);
  let links = 0;
  await scenario((url) => {
    if (isLink(url)) {
      links++;
      return links === 1 ? flakyReply(200, { json: { url: null } }) : noLink();
    }
    return reply(200, { bytes: body });
  }, async () => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    assert.deepEqual(result.missing, []);
    assert.deepEqual(sleeps, [500]);
    assert.equal(links, 2);
  });
}

// A denied IndexedDB (the global getter throws) means memory only: the figure
// is still fetched from the network.
{
  const body = Uint8Array.of(34);
  const sha = await digestOf(body);
  const realDescriptor = Object.getOwnPropertyDescriptor(globalThis, "indexedDB");
  Object.defineProperty(globalThis, "indexedDB", {
    get() {
      throw new Error("denied");
    },
    configurable: true,
  });
  try {
    await scenario((url) => {
      if (isLink(url)) return noLink();
      return reply(200, { bytes: body });
    }, async (calls) => {
      const result = await gather("doc", { "fig.png": sha });
      assert.deepEqual([...result.assets["fig.png"]], [...body]);
      assert.deepEqual(result.missing, []);
      assert.equal(calls.filter((call) => !isLink(call.url)).length, 1);
    });
  } finally {
    if (realDescriptor) Object.defineProperty(globalThis, "indexedDB", realDescriptor);
    else delete globalThis.indexedDB;
  }
}

// The open project's figures are never evicted to make room: with a cap of 100,
// the write of C evicts A (not kept) and leaves B (kept) in place.
{
  const make = (fill) => Uint8Array.of(...new Array(40).fill(fill));
  const a = make(41);
  const b = make(42);
  const c = make(43);
  const [shaA, shaB, shaC] = await Promise.all([a, b, c].map(digestOf));
  const fake = fakeIndexedDB();
  fake.databases.set(ASSET_DATABASE, new Map([
    ["bytes", new Map([[shaA, a], [shaB, b]])],
    ["meta", new Map([[shaA, { size: 40, used: 1 }], [shaB, { size: 40, used: 2 }]])],
  ]));
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: c });
  }, async () => {
    keep([shaB, shaC]);
    await gather("doc", { "c.png": shaC });
    await tick();
    const bytesStore = fake.databases.get(ASSET_DATABASE).get("bytes");
    const metaStore = fake.databases.get(ASSET_DATABASE).get("meta");
    assert.equal(bytesStore.has(shaA), false, "the old entry that is not kept is evicted");
    assert.equal(metaStore.has(shaA), false);
    assert.equal(bytesStore.has(shaB), true, "a kept entry is not evicted");
    assert.equal(metaStore.has(shaB), true);
    assert.equal(bytesStore.has(shaC), true, "the new kept entry stays while the store is within its cap");
  }, { indexedDB: fake, capBytes: 100 });
}

// When the kept entries alone exceed the cap, nothing kept is evicted, and the
// entry just written is not left in the store.
{
  const make = (fill) => Uint8Array.of(...new Array(40).fill(fill));
  const a = new Uint8Array(10).fill(51);
  const b = make(52);
  const c = make(53);
  const d = make(54);
  const [shaA, shaB, shaC, shaD] = await Promise.all([a, b, c, d].map(digestOf));
  const fake = fakeIndexedDB();
  fake.databases.set(ASSET_DATABASE, new Map([
    ["bytes", new Map([[shaA, a], [shaB, b], [shaD, d]])],
    ["meta", new Map([[shaA, { size: 10, used: 0 }], [shaB, { size: 40, used: 1 }], [shaD, { size: 40, used: 2 }]])],
  ]));
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: c });
  }, async () => {
    keep([shaB, shaC, shaD]);
    await gather("doc", { "c.png": shaC });
    await tick();
    const bytesStore = fake.databases.get(ASSET_DATABASE).get("bytes");
    const metaStore = fake.databases.get(ASSET_DATABASE).get("meta");
    assert.equal(bytesStore.has(shaA), false, "the evictable entry goes first");
    assert.equal(bytesStore.has(shaB), true, "kept entries are not evicted");
    assert.equal(bytesStore.has(shaD), true, "kept entries are not evicted");
    assert.equal(bytesStore.has(shaC), false, "the entry just written is dropped when still over the cap");
    assert.equal(metaStore.has(shaC), false, "and its meta is dropped too");
    assert.deepEqual([...metaStore.keys()].sort(), [shaB, shaD].sort());
  }, { indexedDB: fake, capBytes: 100 });
}

// release() clears the kept set: an entry kept before it can be evicted after.
{
  const make = (fill) => Uint8Array.of(...new Array(40).fill(fill));
  const b = make(61);
  const c = make(62);
  const [shaB, shaC] = await Promise.all([b, c].map(digestOf));
  const fake = fakeIndexedDB();
  fake.databases.set(ASSET_DATABASE, new Map([
    ["bytes", new Map([[shaB, b]])],
    ["meta", new Map([[shaB, { size: 40, used: 1 }]])],
  ]));
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: c });
  }, async () => {
    keep([shaB]);
    release();
    await gather("doc", { "c.png": shaC });
    await tick();
    const bytesStore = fake.databases.get(ASSET_DATABASE).get("bytes");
    assert.equal(bytesStore.has(shaB), false, "after release the entry is no longer kept, so it is evicted");
    assert.equal(bytesStore.has(shaC), true);
  }, { indexedDB: fake, capBytes: 100 });
}

// After a sign-out nothing is written, and the store is not even opened. This
// must stay the last test: the sign-out is for the rest of this process.
{
  const body = Uint8Array.of(17);
  const sha = await digestOf(body);
  const fake = fakeIndexedDB();
  markSignedOut();
  await scenario((url) => {
    if (isLink(url)) return noLink();
    return reply(200, { bytes: body });
  }, async () => {
    const result = await gather("doc", { "fig.png": sha });
    assert.deepEqual([...result.assets["fig.png"]], [...body]);
    await tick();
    assert.equal(fake.opens, 0);
    assert.equal(fake.databases.size, 0);
  }, { indexedDB: fake });
}

console.log("figures: data URLs for images, blob URLs for PDFs, store, retries and fallbacks");
