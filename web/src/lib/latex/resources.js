// The shared TeX Live cache: Cache Storage, namespaced by release digest,
// with sha256 verification on the way in, plus one working set per engine
// kind: the resolver answers and files a kind actually used.
//
// This deliberately duplicates `../cache.js` rather than reusing it. That
// file caches document assets under their bare URL and never verifies bytes
// (the server is trusted for those); this one caches distribution files
// fetched from a mirror a compromised or misconfigured server could poison,
// and the spec asks for verification and eviction recovery that bare-URL
// caching does not need. The two caches are also namespaced apart on purpose
// ("librepaper" vs "librepaper-latex-*") so `clear()` here can never touch a
// project's rendered pages, and vice versa.
//
// Everything here runs unbundled under Node for `latex-resources.mjs`, so it
// only touches `caches`, `fetch`, `crypto.subtle` and `navigator.storage`
// through the global object -- Node 24 provides the first three natively and
// the check fakes the rest.
//
// `fetchVerified` and `prefetch` take a `release` alongside the shapes the
// interface spec lists (section 2.5): `worker.js` (this package) owns one
// release at a time and is the only caller, so the release travels as an
// explicit argument rather than through hidden module state -- easier to
// test and it makes "which cache" visible at every call site.

import { sha256Hex } from "../digest.js";

const PREFIX = "librepaper-latex-";
/// The engine cache from before working sets: one shared Cache Storage name
/// holding every bundle, never scoped to a release.
const LEGACY_BUNDLES = "librepaper-bundles";
const WORKING_SET_PREFIX = "https://librepaper.invalid/working-set/";

/** @typedef {{digest?: string}} LatexRelease */
/** @typedef {{sha256?: string, size?: number, signal?: AbortSignal}} VerificationMetadata */
/** @typedef {{url: string, sha256: string, size: number, scope?: string}} ResourceEntry */

/// The Cache Storage name for one release: short (16 hex chars of the
/// release's own digest) so it stays readable in devtools, and derived from
/// the digest rather than the id so a re-signed manifest entry with the same
/// id but different bytes gets its own cache instead of silently reusing a
/// stale one.
/** @param {LatexRelease} release */
export function namespace(release) {
  const digest = release?.digest ?? "";
  return `${PREFIX}${digest.slice(0, 16)}`;
}

/** @param {VerificationMetadata} [metadata] */
function requiredMetadata({ sha256, size } = {}) {
  if (typeof sha256 !== "string" || !/^[a-f0-9]{64}$/.test(sha256)) {
    throw new Error("manifest metadata must include a valid sha256 digest");
  }
  if (!Number.isSafeInteger(size) || size < 0) {
    throw new Error("manifest metadata must include a non-negative size");
  }
  return { sha256, size };
}

/** @param {LatexRelease} release */
async function openStore(release) {
  if (!globalThis.caches?.open) return null;
  return caches.open(namespace(release)).catch(() => null);
}

/// Fetch a URL through Cache Storage, verified. A cache hit is trusted only
/// after its sha256 matches: an entry that fails (partial write from a
/// crashed tab, storage corruption) is deleted and refetched rather than
/// served broken, because a corrupt `.fmt` silently poisoning every compile
/// in the session is worse than one slow refetch. A network error or a
/// non-2xx response throws and is never written to the cache: a transient
/// failure must not become a permanent "this file does not exist" entry.
/** @param {LatexRelease} release @param {string|URL} url @param {VerificationMetadata} [metadata] */
export async function fetchVerified(release, url, { sha256, size, signal } = {}) {
  ({ sha256, size } = requiredMetadata({ sha256, size }));
  const store = await openStore(release);
  if (store) {
    const hit = await store.match(url).catch(() => null);
    if (hit) {
      const bytes = new Uint8Array(await hit.clone().arrayBuffer());
      const sizeOk = bytes.length === size;
      const shaOk = (await sha256Hex(bytes)) === sha256;
      if (sizeOk && shaOk) return hit;
      // Corrupt or stale: discard and fall through to a real fetch.
      await store.delete(url).catch(() => {});
    }
  }
  const response = await fetch(url, { signal });
  if (!response.ok) {
    throw new Error(`fetch ${url} failed: ${response.status}`);
  }
  const bytes = new Uint8Array(await response.clone().arrayBuffer());
  if (bytes.length !== size) {
    throw new Error(`fetch ${url} failed size verification (expected ${size}, got ${bytes.length})`);
  }
  const actual = await sha256Hex(bytes);
  if (actual !== sha256) {
    throw new Error(`fetch ${url} failed sha256 verification (expected ${sha256}, got ${actual})`);
  }
  const stored = new Response(bytes, { headers: { "content-length": String(bytes.length) } });
  if (store) {
    await store.put(url, stored.clone()).catch(() => {
      /* a full or refused quota is not a reason to fail the compile */
    });
  }
  return stored;
}

/// Parallel prefetch, six requests at a time -- matching the browser's own
/// practical connection limit per origin, so a larger concurrency would not
/// actually parallelize further, just queue inside the browser instead of
/// here where progress can still be reported per completion.
const PREFETCH_CONCURRENCY = 6;

/** @param {LatexRelease} release @param {ResourceEntry[]} entries @param {(progress: {done: number,total: number,scope: string}) => void} [onProgress] */
export async function prefetch(release, entries, onProgress) {
  const list = Array.isArray(entries) ? entries : [];
  const total = list.length;
  let done = 0;
  const report = (scope) => onProgress?.({ done, total, scope: scope ?? "resources" });
  report();
  let index = 0;
  const results = new Array(total);
  const worker = async () => {
    for (;;) {
      const i = index++;
      if (i >= total) return;
      const entry = list[i];
      results[i] = await fetchVerified(release, entry.url, { sha256: entry.sha256, size: entry.size });
      done++;
      report(entry.scope);
    }
  };
  await Promise.all(Array.from({ length: Math.min(PREFETCH_CONCURRENCY, total) }, worker));
  return results;
}

/// Bytes held across every `librepaper-latex-*` cache. Sums `content-length`
/// when a response carries it (cheap: no body read) and falls back to
/// reading the body only when it does not; `fetchVerified` above always
/// stores a response with the header set, so the fallback exists only for
/// safety against a differently-produced entry.
export async function size() {
  if (!globalThis.caches?.keys) return 0;
  const names = (await caches.keys()).filter((name) => name.startsWith(PREFIX));
  let total = 0;
  for (const name of names) {
    const store = await caches.open(name);
    for (const request of await store.keys()) {
      const response = await store.match(request);
      if (!response) continue;
      const length = Number(response.headers.get("content-length"));
      if (Number.isFinite(length) && length > 0) {
        total += length;
      } else {
        total += (await response.clone().arrayBuffer()).byteLength;
      }
    }
  }
  return total;
}

/// Deletes every `librepaper-latex-*` cache and never touches anything else: a project's rendered pages
/// live in the plain `librepaper` cache from `cache.js`, a different name
/// entirely, so clearing compiler resources can never delete source
/// documents.
export async function clear() {
  if (!globalThis.caches?.keys) return;
  const names = (await caches.keys()).filter((name) => name.startsWith(PREFIX));
  await Promise.all(names.map((name) => caches.delete(name)));
}

let persisted = null;

/// `navigator.storage.persist()`, asked once per page load and never
/// repeated: a refusal is not retried on every compile, and a grant does not
/// need re-asking. Refusal is tolerated -- the distribution is simply subject
/// to eviction under storage pressure, which `fetchVerified`'s corrupt/missing
/// recovery already handles.
export function persist() {
  if (persisted) return persisted;
  persisted = Promise.resolve()
    .then(() => navigator?.storage?.persist?.() ?? false)
    .catch(() => false);
  return persisted;
}

/// A working set is the resolver answers one engine kind gave (`resolutions`)
/// and a tar of the files they point at. It lives in the release's own cache,
/// so `size()` counts it and `clear()` removes it with everything else.
/// Body: 4-byte little-endian JSON length, the JSON, then the tar.
/** @param {string} kind */
function workingSetKey(kind) {
  return `${WORKING_SET_PREFIX}${kind}`;
}

/** @param {LatexRelease} release @param {string} kind @param {{resolutions: unknown[], files: Uint8Array}} set */
export async function writeWorkingSet(release, kind, { resolutions, files }) {
  const store = await openStore(release);
  if (!store) return;
  const json = new TextEncoder().encode(JSON.stringify(resolutions));
  const body = new Uint8Array(4 + json.length + files.length);
  new DataView(body.buffer).setUint32(0, json.length, true);
  body.set(json, 4);
  body.set(files, 4 + json.length);
  const response = new Response(body, {
    headers: {
      "content-length": String(body.length),
      "x-librepaper-sha256": await sha256Hex(body),
    },
  });
  await store.put(workingSetKey(kind), response).catch(() => {
    /* a full or refused quota only costs the next start its warm cache */
  });
}

/// Storage is best effort: any failure reads as "no working set". An entry
/// that fails verification is deleted so it is not re-read on every start.
/** @param {LatexRelease} release @param {string} kind @returns {Promise<{resolutions: unknown[], files: Uint8Array}|null>} */
export async function readWorkingSet(release, kind) {
  try {
    const store = await openStore(release);
    if (!store) return null;
    const key = workingSetKey(kind);
    const hit = await store.match(key);
    if (!hit) return null;
    const body = new Uint8Array(await hit.arrayBuffer());
    let resolutions = null;
    let files = null;
    if ((await sha256Hex(body)) === hit.headers.get("x-librepaper-sha256") && body.length >= 4) {
      const length = new DataView(body.buffer, body.byteOffset, body.byteLength).getUint32(0, true);
      if (4 + length <= body.length) {
        try {
          resolutions = JSON.parse(new TextDecoder().decode(body.subarray(4, 4 + length)));
          files = body.subarray(4 + length);
        } catch {
          resolutions = null;
        }
      }
    }
    if (!Array.isArray(resolutions) || !files) {
      await store.delete(key).catch(() => {});
      return null;
    }
    return { resolutions, files };
  } catch {
    return null;
  }
}

const evictions = new Map();

/// Drops the caches of every other release, and `librepaper-bundles`, which
/// is the pre-working-set engine cache and was never release-scoped. Once per
/// namespace for the page's lifetime; the promise never rejects.
/** @param {LatexRelease} release */
export function evictOtherReleases(release) {
  const current = namespace(release);
  if (!evictions.has(current)) {
    evictions.set(current, (async () => {
      if (!globalThis.caches?.keys) return;
      const names = await caches.keys();
      await Promise.all(names
        .filter((name) => (name.startsWith(PREFIX) && name !== current) || name === LEGACY_BUNDLES)
        .map((name) => caches.delete(name)));
    })().catch(() => {}));
  }
  return evictions.get(current);
}
