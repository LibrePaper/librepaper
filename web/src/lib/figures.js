// The document's figures, as this browser holds them.
//
// A figure is not in the shared document: what travels there is a path and the
// digest of the bytes at it. The bytes come from the store, once, and stay --
// content-addressed, so a figure fetched for one version of a document is the
// same figure for every later one, and a re-render on a keystroke reads it
// from memory rather than from anywhere.
//
// Two forms are needed, because the two renderers want different things. Typst
// takes the bytes and writes the image into the page itself. Markdown produces
// HTML a browser will fetch from, so it takes a data URL -- never the route
// the bytes came from, which on a private document carries a credential and
// would put it inside a rendered page. A data URL, unlike a blob URL made here,
// loads in the document frame, which lives on another origin, and an SVG opened
// from one in a tab of its own runs in an opaque origin rather than as the app.
// PDFs stay blob URLs: the reader's figure view shows them in an <object>, and
// browsers render them in a PDF viewer that is isolated from the page.
//
// Behind the memory map sits a persistent store (IndexedDB, keyed by digest)
// so a figure survives a reload. Bytes are fetched from a signed link on the
// object store when the server offers one, and from the byte route otherwise
// (spec transport 5 and 6.2).

import { ASSET_DATABASE, localDataForgotten } from "./local-data.js";

/// How much the persistent store may hold, least recently used evicted first.
const ASSET_STORE_BYTES = 256 * 1024 * 1024;
// Two object stores, so that eviction reads only the small metadata and never
// the bytes: `bytes` (digest -> Uint8Array) and `meta` (digest -> {size, used}).
const BYTES = "bytes";
const META = "meta";
/// Retries for network errors and 5xx from our server.
const BACKOFF = [500, 1000, 2000];
/// Retries for a 429 that says the server is out of transfer or work permits.
const THROTTLE_RETRIES = 5;

const bytes = new Map(); // digest -> Uint8Array
const urls = new Map(); // digest -> data: URL or blob: URL (PDFs only)
const kept = new Set(); // digests of the open project, never evicted to make room
const inFlight = new Map(); // digest -> Promise
const scopes = new Map();
let generation = 0;
let nextScope = 0;
// Set when a link is refused or a bucket fails as a network error; the tab
// then uses the byte route for the rest of its life.
let useByteRoute = false;
// The open database, or a promise of it. Not kept for a signed-out tab.
let database = null;
// Digests whose last use has been written in this tab (at most once each).
const touched = new Set();

// Test-only hooks. `sleep` and `random` replace the retry waits and jitter;
// `indexedDB` replaces the factory (null means none); `capBytes` replaces the
// cap. Passing undefined for a key restores its default.
const hooks = { sleep: undefined, random: undefined, indexedDB: undefined, capBytes: undefined };

/** @test-only */
export function __setAssetTestHooks(overrides = {}) {
  Object.assign(hooks, overrides);
}

/** Test-only: forgets the tab's store state, as if the module were fresh. The
 * persistent store itself is not deleted. */
export function __resetAssetTestState() {
  release();
  kept.clear();
  useByteRoute = false;
  database = null;
  touched.clear();
}

const defaultSleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const sleep = (ms) => (hooks.sleep ?? defaultSleep)(ms);
const jitter = () => (hooks.random ?? Math.random)() * 250;
const storeCap = () => hooks.capBytes ?? ASSET_STORE_BYTES;
// Reading the global can throw where storage is denied; that is no store.
function idbFactory() {
  if (hooks.indexedDB !== undefined) return hooks.indexedDB;
  try {
    return globalThis.indexedDB ?? null;
  } catch {
    return null;
  }
}

function scopedKey(slug, sha, headers) {
  const entries = headers instanceof Headers ? [...headers.entries()] : Object.entries(headers || {});
  const fingerprint = JSON.stringify([slug, entries.sort(([a], [b]) => a.localeCompare(b))]);
  if (!scopes.has(fingerprint)) scopes.set(fingerprint, ++nextScope);
  return `${scopes.get(fingerprint)}:${sha}`;
}

const isDigest = (sha) => /^[0-9a-f]{64}$/.test(sha);

async function digestOf(body) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", body))].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function requestResult(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error || new Error("asset store request failed"));
  });
}

// Collects every row of a cursor, oldest first, as the store's sizes.
function collectRows(req) {
  return new Promise((resolve, reject) => {
    const rows = [];
    req.onsuccess = () => {
      const cursor = req.result;
      if (!cursor) {
        resolve(rows);
        return;
      }
      rows.push({ key: cursor.primaryKey, size: cursor.value.size });
      cursor.continue();
    };
    req.onerror = () => reject(req.error || new Error("asset store cursor failed"));
  });
}

// Opens the store, or answers null: for a signed-out tab, a browser without
// IndexedDB, or a refused open. Nothing here throws.
function openStore() {
  if (localDataForgotten()) return Promise.resolve(null);
  if (database) return database;
  const factory = idbFactory();
  if (!factory) return Promise.resolve(null);
  database = new Promise((resolve, reject) => {
    const open = factory.open(ASSET_DATABASE, 1);
    open.onupgradeneeded = () => {
      const db = open.result;
      if (!db.objectStoreNames.contains(BYTES)) db.createObjectStore(BYTES);
      if (!db.objectStoreNames.contains(META)) db.createObjectStore(META).createIndex("used", "used");
    };
    open.onsuccess = () => {
      const db = open.result;
      // A sign-out in another tab deletes this database. Closing here lets
      // that delete finish, and the next call reopens a fresh connection.
      db.onversionchange = () => {
        db.close();
        database = null;
      };
      resolve(db);
    };
    open.onerror = () => reject(open.error || new Error("asset store is unavailable"));
  }).catch(() => null);
  return database;
}

/// The bytes stored for a digest, verified, or null. An entry that fails its
/// digest is deleted so it is fetched again.
async function readStored(sha) {
  const db = await openStore();
  if (!db) return null;
  let stored;
  try {
    stored = await requestResult(db.transaction(BYTES, "readonly").objectStore(BYTES).get(sha));
  } catch {
    return null;
  }
  if (!stored) return null;
  const body = new Uint8Array(stored);
  let actual;
  try {
    actual = await digestOf(body);
  } catch {
    return null;
  }
  if (actual !== sha) {
    await deleteStored(sha);
    return null;
  }
  void touch(sha);
  return body;
}

async function deleteStored(sha) {
  const db = await openStore();
  if (!db || localDataForgotten()) return;
  try {
    const tx = db.transaction([BYTES, META], "readwrite");
    tx.objectStore(BYTES).delete(sha);
    tx.objectStore(META).delete(sha);
  } catch {
    /* the entry stays; the next read will check it again */
  }
}

/// Writes this tab's last use of an entry, once per digest.
async function touch(sha) {
  if (touched.has(sha)) return;
  touched.add(sha);
  const db = await openStore();
  if (!db || localDataForgotten()) return;
  try {
    const meta = db.transaction(META, "readwrite").objectStore(META);
    const record = await requestResult(meta.get(sha));
    if (record) meta.put({ ...record, used: Date.now() }, sha);
  } catch {
    /* the entry keeps its old place in the order */
  }
}

/// Stores verified bytes, then evicts least recently used entries until the
/// store is within its cap. Entries of the open project (`kept`) are never
/// evicted. An asset larger than the cap is not stored.
async function writeStored(sha, body) {
  if (body.byteLength > storeCap()) return;
  const db = await openStore();
  if (!db || localDataForgotten()) return;
  try {
    const tx = db.transaction([BYTES, META], "readwrite");
    tx.objectStore(BYTES).put(body, sha);
    const meta = tx.objectStore(META);
    meta.put({ size: body.byteLength, used: Date.now() }, sha);
    touched.add(sha);
    const rows = await collectRows(meta.index("used").openCursor());
    let total = rows.reduce((sum, row) => sum + row.size, 0);
    for (const row of rows) {
      if (total <= storeCap()) break;
      if (kept.has(row.key) || row.key === sha) continue;
      tx.objectStore(BYTES).delete(row.key);
      meta.delete(row.key);
      total -= row.size;
    }
    // Every evictable entry is gone and the store is still over its cap: the
    // project is bigger than the cap. The entry just written is dropped too,
    // so the rest of the project comes from the network on the next cold tab
    // rather than the store holding a part of it past its cap.
    if (total > storeCap()) {
      tx.objectStore(BYTES).delete(sha);
      meta.delete(sha);
    }
  } catch {
    /* a quota refusal, or a database deleted under this tab: memory only */
  }
}

function retryAfterMs(response) {
  const raw = response.headers?.get?.("Retry-After");
  const seconds = raw === null || raw === undefined ? NaN : Number(raw);
  return (Number.isFinite(seconds) && seconds >= 0 ? seconds : 1) * 1000;
}

async function reasonOf(response) {
  try {
    return (await response.json())?.reason;
  } catch {
    return undefined;
  }
}

/// A request to our server, with the retry table of spec 5.2. `read`, when
/// given, reads the body of an OK response, and is part of the retried request:
/// a body that fails to read (a connection dropped mid-download) is retried
/// exactly like a network error. Returns `{ response, body }`, where body is
/// `read`'s result for an OK response and undefined otherwise; the caller checks
/// the response. A network error that runs out of retries throws.
async function askServer(url, init, read) {
  let failures = 0; // network errors, failed body reads and 5xx, shared
  let throttled = 0;
  for (;;) {
    let response;
    let body;
    try {
      response = await fetch(url, init);
      if (response.ok && read) body = await read(response);
    } catch (error) {
      if (failures >= BACKOFF.length) throw error;
      await sleep(BACKOFF[failures++] + jitter());
      continue;
    }
    if (response.status === 429) {
      const reason = await reasonOf(response);
      if ((reason === "transfer_concurrency" || reason === "work_concurrency") && throttled < THROTTLE_RETRIES) {
        throttled++;
        await sleep(retryAfterMs(response) + jitter());
        continue;
      }
      return { response, body };
    }
    if (response.status >= 500 && failures < BACKOFF.length) {
      await sleep(BACKOFF[failures++] + jitter());
      continue;
    }
    return { response, body };
  }
}

/// The signed link for a digest: `{ url }`, where url is null when the server
/// offers none, or `{ failed: true }` when the server has no link route (404 or
/// 405, an older or stand-in server). Then this asset, and the rest of the
/// tab, use the byte route. Any other refusal throws, after askServer's
/// retries: the byte route would meet the same refusal from the same server,
/// and trying it too would only add load to a server that is already busy.
async function requestLink(slug, sha, headers) {
  const { response, body } = await askServer(
    `/api/documents/${slug}/assets/${sha}/link`,
    { credentials: "same-origin", headers, cache: "no-store" },
    (r) => r.json(),
  );
  if (response.status === 404 || response.status === 405) {
    useByteRoute = true;
    return { failed: true };
  }
  if (!response.ok) throw new Error(`could not fetch a figure link (${response.status})`);
  const { url } = body;
  return { url: url ?? null };
}

/// The bytes from the bucket, or null when the byte route should be used.
async function fetchFromBucket(slug, sha, headers) {
  const first = await requestLink(slug, sha, headers);
  if (first.failed) return null;
  if (first.url === null) {
    useByteRoute = true;
    return null;
  }
  let link = first.url;
  let reissued = false;
  for (;;) {
    let response;
    try {
      response = await fetch(link, { cache: "no-store", credentials: "omit" });
      if (response.ok) return new Uint8Array(await response.arrayBuffer());
    } catch {
      // A network or CORS error from the bucket: this tab uses the byte route.
      useByteRoute = true;
      return null;
    }
    if (response.status !== 403 || reissued) return null;
    reissued = true;
    let again;
    try {
      again = await requestLink(slug, sha, headers);
    } catch {
      return null;
    }
    if (again.failed) return null;
    if (again.url === null) {
      useByteRoute = true;
      return null;
    }
    link = again.url;
  }
}

/// The bytes from the server, by the link when there is one and by the byte
/// route otherwise. Not verified here.
async function fetchFromServer(slug, sha, headers) {
  if (!useByteRoute) {
    const fromBucket = await fetchFromBucket(slug, sha, headers);
    if (fromBucket) return fromBucket;
  }
  const { response, body } = await askServer(
    `/api/documents/${slug}/assets/${sha}`,
    { credentials: "same-origin", headers, cache: "no-store" },
    (r) => r.arrayBuffer(),
  );
  if (!response.ok) throw new Error(`could not fetch a figure (${response.status})`);
  return new Uint8Array(body);
}

/// The bytes of a figure, fetched once per browser and kept.
async function fetchOne(slug, sha, headers) {
  const key = scopedKey(slug, sha, headers);
  const captured = generation;
  if (bytes.has(key)) return bytes.get(key);
  if (inFlight.has(key)) return inFlight.get(key);
  const wanted = (async () => {
    const stored = isDigest(sha) ? await readStored(sha) : null;
    let body = stored;
    if (!body) {
      body = await fetchFromServer(slug, sha, headers);
      if (generation !== captured) throw new Error("Asset authorization changed");
      if (isDigest(sha) && (await digestOf(body)) !== sha) throw new Error("Captured asset failed its integrity check");
    }
    if (generation !== captured) throw new Error("Asset authorization changed");
    bytes.set(key, body);
    if (!stored && isDigest(sha)) void writeStored(sha, body);
    return body;
  })();
  inFlight.set(key, wanted);
  try {
    return await wanted;
  } finally {
    if (inFlight.get(key) === wanted) inFlight.delete(key);
  }
}

/// Encode bytes as base64 in the browser.
export function toBase64(bytes) {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return btoa(binary);
}

/// What MIME type to use for these bytes. The store answers every figure as
/// octet-stream -- it knows a digest and not a filename -- so the type is
/// worked out here, from the name the document gave it.
export function typeOf(path) {
  const lower = path.toLowerCase();
  if (lower.endsWith(".png")) return "image/png";
  if (lower.endsWith(".jpg") || lower.endsWith(".jpeg")) return "image/jpeg";
  if (lower.endsWith(".gif")) return "image/gif";
  if (lower.endsWith(".webp")) return "image/webp";
  if (lower.endsWith(".svg")) return "image/svg+xml";
  if (lower.endsWith(".pdf")) return "application/pdf";
  return "application/octet-stream";
}

/// Fetches every figure a document names and returns them in both forms.
/// `digests` is the path-to-digest map the shared document carries.
///
/// A figure that cannot be fetched is left out rather than failing the render:
/// the rest of the document is still worth showing, and the image is broken in
/// the page, which is what a missing figure looks like everywhere else.
/** @param {string} slug @param {Record<string, string>} digests @param {HeadersInit} [headers] @param {{strict?: boolean, urls?: boolean}} [options] @returns {Promise<{assets: Record<string, Uint8Array>, urls: Record<string, string>, missing: string[]}>} */
export async function gather(slug, digests, headers = {}, { strict = false, urls: wantUrls = true } = {}) {
  const wanted = Object.entries(digests || {});
  const got = await Promise.all(
    wanted.map(async ([path, sha]) => {
      try {
        const key = scopedKey(slug, sha, headers);
        const captured = generation;
        const body = await fetchOne(slug, sha, headers);
        if (generation !== captured) throw new Error("Asset authorization changed");
        if (!wantUrls) return [path, body, null];
        if (!urls.has(key)) {
          const type = typeOf(path);
          let url;
          if (type === "application/pdf") {
            url = URL.createObjectURL(new Blob([body], { type }));
          } else {
            url = `data:${type};base64,${toBase64(body)}`;
          }
          // Keep the immutable store identity in the fragment, which is
          // ignored by the resource fetch but available to the injected
          // document agent.
          urls.set(key, `${url}#librepaper-asset=${encodeURIComponent(sha)}`);
        }
        return [path, body, urls.get(key)];
      } catch {
        return null;
      }
    }),
  );
  /** @type {Record<string, Uint8Array>} */
  const assets = {};
  /** @type {Record<string, string>} */
  const where = {};
  /** @type {string[]} */
  const missing = [];
  for (const one of got) {
    if (!one) continue;
    const [path, body, url] = one;
    assets[path] = body;
    if (url) where[path] = url;
  }
  for (const [path] of wanted) {
    if (!Object.prototype.hasOwnProperty.call(assets, path)) missing.push(path);
  }
  if (strict && missing.length) {
    throw Object.assign(new Error(`could not fetch figure${missing.length === 1 ? "" : "s"}: ${missing.join(", ")}`), { missing });
  }
  return { assets, urls: where, missing };
}

/// Whether every figure a document names is already here, which is what says a
/// render can go ahead without waiting. A render that went ahead without them
/// would produce a page with holes in it and then be replaced a moment later,
/// which reads as a flicker.
export function ready(digests, slug = "", headers = {}) {
  return Object.values(digests || {}).every((sha) => bytes.has(scopedKey(slug, sha, headers)));
}

/// The open project's figures. The persistent store never evicts these to
/// make room: a project is loaded whole, and switching files inside it must
/// not fetch again. Called with every digest of the current tree.
export function keep(digests) {
  kept.clear();
  for (const sha of digests) kept.add(sha);
}

/// Forgets everything, for a page leaving a document. The blob URLs are
/// revoked: each one holds its bytes alive in the browser until it is.
/// Data URLs do not need to be revoked. The persistent store is kept, and so
/// is no longer protected: the kept set is cleared.
export function release() {
  generation++;
  kept.clear();
  for (const url of urls.values()) {
    const base = url.split("#", 1)[0];
    if (base.startsWith("blob:")) URL.revokeObjectURL(base);
  }
  urls.clear();
  bytes.clear();
  inFlight.clear();
  scopes.clear();
}
