// The shared TeX Live cache: Cache Storage, namespaced by release digest,
// with sha256 verification on the way in and a bounded record of what a
// project has actually pulled through it.
//
// This deliberately duplicates `../cache.js` rather than reusing it. That
// file caches document assets under their bare URL and never verifies bytes
// (the server is trusted for those); this one caches distribution files
// fetched from a mirror a compromised or misconfigured server could poison,
// and the spec asks for verification and eviction recovery that bare-URL
// caching does not need. The two caches are also namespaced apart on purpose
// ("komodoc" vs "komodoc-latex-*") so `clear()` here can never touch a
// project's rendered pages, and vice versa.
//
// Everything here runs unbundled under Node for `latex-resources.mjs`, so it
// only touches `caches`, `fetch`, `crypto.subtle`, `localStorage` and
// `navigator.storage` through the global object -- Node 24 provides the
// first three natively and the check fakes the rest.
//
// `fetchVerified` and `prefetch` take a `release` alongside the shapes the
// interface spec lists (section 2.5): `worker.js` (this package) owns one
// release at a time and is the only caller, so the release travels as an
// explicit argument rather than through hidden module state -- easier to
// test and it makes "which cache" visible at every call site.

const PREFIX = "komodoc-latex-";
const REMEMBER_KEY = "komodoc-latex-used";
const REMEMBER_LIMIT = 2000;

/// The Cache Storage name for one release: short (16 hex chars of the
/// release's own digest) so it stays readable in devtools, and derived from
/// the digest rather than the id so a re-signed manifest entry with the same
/// id but different bytes gets its own cache instead of silently reusing a
/// stale one.
export function namespace(release) {
  const digest = release?.digest ?? "";
  return `${PREFIX}${digest.slice(0, 16)}`;
}

function toHex(buffer) {
  return Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, "0")).join("");
}

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return toHex(digest);
}

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
export async function fetchVerified(release, url, { sha256, size, signal } = {}) {
  const store = await openStore(release);
  if (store) {
    const hit = await store.match(url).catch(() => null);
    if (hit) {
      const bytes = new Uint8Array(await hit.clone().arrayBuffer());
      const sizeOk = !(typeof size === "number" && size > 0) || bytes.length === size;
      const shaOk = !sha256 || (await sha256Hex(bytes)) === sha256;
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
  if (sha256) {
    const actual = await sha256Hex(bytes);
    if (actual !== sha256) {
      throw new Error(`fetch ${url} failed sha256 verification (expected ${sha256}, got ${actual})`);
    }
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

/// Bytes held across every `komodoc-latex-*` cache. Sums `content-length`
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

/// Deletes every `komodoc-latex-*` cache -- which includes the biber VM's
/// static resources, namespaced under the same prefix by convention (see
/// `vm.js`) -- and never touches anything else: a project's rendered pages
/// live in the plain `komodoc` cache from `cache.js`, a different name
/// entirely, so clearing compiler resources can never delete source
/// documents.
export async function clear() {
  if (!globalThis.caches?.keys) return;
  const names = (await caches.keys()).filter((name) => name.startsWith(PREFIX));
  await Promise.all(names.map((name) => caches.delete(name)));
  try {
    localStorage?.removeItem?.(REMEMBER_KEY);
  } catch {
    /* no localStorage (private window, Node) */
  }
}

/// Whether the texlive keys a project has actually used (recorded via
/// `remember`) are all present in this release's cache. `keys` are
/// `{key, url}` pairs -- `key` is the manifest key (`pdftex/26/amsmath.sty`,
/// used for reporting) and `url` is the fetchable mirror URL that was cached
/// under; a bare string is accepted too and used as both.
export async function readiness(release, keys) {
  const list = Array.isArray(keys) ? keys : [];
  const store = await openStore(release);
  const missing = [];
  for (const entry of list) {
    const key = typeof entry === "string" ? entry : entry.key;
    const url = typeof entry === "string" ? entry : entry.url;
    const hit = store && url ? await store.match(url).catch(() => null) : null;
    if (!hit) missing.push(key);
  }
  return { ready: missing.length === 0, missing };
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

/// Records that a project used a texlive key, bounded to `REMEMBER_LIMIT`
/// keys per release so an old, abandoned project cannot grow this list
/// forever. Oldest entries are dropped first (insertion order), matching the
/// intuition that recently-used keys are the ones worth keeping for offline
/// readiness.
export function remember(release, key) {
  let store;
  try {
    const raw = localStorage?.getItem?.(REMEMBER_KEY);
    store = raw ? JSON.parse(raw) : {};
  } catch {
    store = {};
  }
  const ns = namespace(release);
  const list = Array.isArray(store[ns]) ? store[ns] : [];
  if (!list.includes(key)) list.push(key);
  while (list.length > REMEMBER_LIMIT) list.shift();
  store[ns] = list;
  try {
    localStorage?.setItem?.(REMEMBER_KEY, JSON.stringify(store));
  } catch {
    /* storage full or unavailable: remembering is best-effort */
  }
}

/// The keys remembered for a release (used by callers that want "the
/// resources this project has actually used" without tracking it
/// themselves, e.g. to build the `keys` argument to `readiness`).
export function remembered(release) {
  try {
    const raw = localStorage?.getItem?.(REMEMBER_KEY);
    const store = raw ? JSON.parse(raw) : {};
    return Array.isArray(store[namespace(release)]) ? store[namespace(release)] : [];
  } catch {
    return [];
  }
}
