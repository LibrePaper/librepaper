// The last page this browser drew for a document, kept so the next visit has
// something to show.
//
// Opening a project used to mean an empty pane until the engine had loaded
// and the source had compiled -- seconds for LaTeX, and the first of those
// seconds is spent fetching a WebAssembly engine that has not started
// rendering anything yet. Nothing about that wait is informative: the page
// that was on screen when the document was last closed is almost always the
// page that is about to be drawn again.
//
// So the payload handed to the frame is kept here, under the identity of the
// source it was rendered from. On the next visit it goes straight to the
// frame, and the compile that follows replaces it. When that identity still
// matches the source, the cached page is not stale at all -- it is the same
// render, and only its provenance is older.
//
// Nothing here throws. A browser without IndexedDB, or one in a private
// window, simply draws nothing until its first compile, which is what every
// browser did before this existed.

const DATABASE = "librepaper-preview-v1";
const VERSION = 1;
const PAGES = "pages";
/// How many documents keep a page. A reader moves between a handful of
/// projects; the rest are worth less than the space they would hold.
const KEPT = 12;
/// And how much they may hold in total. A rendered paper is a megabyte or
/// two, and a browser that refuses a write when its quota is gone would
/// otherwise start failing at whatever unrelated thing asked next.
const CEILING = 96 * 1024 * 1024;
/// A compile finishes every few seconds while somebody is typing, and none of
/// those pages is worth a write of its own. The newest one wins after a
/// pause, which is the same shape the checkpoint and the publication use.
const SETTLE_MS = 5_000;

let opened = null;
const pending = new Map();
const timers = new Map();

function result(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("preview cache request failed"));
  });
}

function open() {
  if (opened) return opened;
  opened = new Promise((resolve, reject) => {
    if (!globalThis.indexedDB) {
      reject(new Error("this browser keeps no preview cache"));
      return;
    }
    const request = indexedDB.open(DATABASE, VERSION);
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(PAGES)) {
        database.createObjectStore(PAGES, { keyPath: "slug" });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("preview cache is unavailable"));
  }).catch(() => null);
  return opened;
}

export function bytesOf(record) {
  if (record?.kind === "pdf") return record.bytes?.byteLength || 0;
  // A JavaScript string is UTF-16 in the store, so its length is not its
  // bytes. Two per unit is the honest upper bound and the one worth holding
  // a budget against.
  return (record?.html?.length || 0) * 2;
}

/// The pages to let go of, newest first until both bounds are met: at most
/// `KEPT` documents, and at most `CEILING` bytes between them. A page that is
/// on its own bigger than the ceiling is still kept when it is the newest --
/// dropping it would mean a document that can never cache its page at all.
///
/// Separated from the write so the rule can be read and tested without a
/// database: this is the whole of the policy, and the transaction below is
/// the whole of the mechanism.
export function pagesToDrop(records, { kept = KEPT, ceiling = CEILING } = {}) {
  const ordered = [...records].sort((a, b) => (b.savedAt || 0) - (a.savedAt || 0));
  const dropped = [];
  let held = 0;
  for (const [at, record] of ordered.entries()) {
    held += bytesOf(record);
    if (at >= kept || (at > 0 && held > ceiling)) dropped.push(record.slug);
  }
  return dropped;
}

/// Drops the least recently drawn pages until what is kept is within both
/// bounds. Runs inside the caller's transaction so a reader who never closes
/// a tab still cannot grow this without limit.
async function evict(store) {
  const all = await result(store.getAll()).catch(() => []);
  for (const slug of pagesToDrop(all)) store.delete(slug);
}

async function write(slug) {
  const record = pending.get(slug);
  pending.delete(slug);
  timers.delete(slug);
  if (!record) return;
  const database = await open();
  if (!database) return;
  try {
    const transaction = database.transaction(PAGES, "readwrite");
    const store = transaction.objectStore(PAGES);
    store.put(record);
    await evict(store);
  } catch {
    // A quota refusal, a database deleted under this tab: the next visit
    // renders from source, which is what it would have done anyway.
  }
}

/// Keeps the page the frame was just given. `identity` is the digest of the
/// source it was rendered from, so the next visit can say whether it is
/// still the current page or only the last one.
/// What is written for a drawn page. Separated for the same reason as the
/// eviction rule: what a record holds is worth reading without a database in
/// the way.
export function pageRecord(slug, payload, identity, now = Date.now()) {
  if (!slug || !payload?.kind) return null;
  return payload.kind === "pdf"
    // A detached buffer cannot be stored, and the frame is handed a fresh
    // one on every transfer, so this takes its own copy.
    ? { slug, kind: "pdf", identity, bytes: new Uint8Array(payload.bytes || []).slice(), savedAt: now }
    : { slug, kind: "html", identity, html: String(payload.html || ""), savedAt: now };
}

export function rememberPreview(slug, payload, identity) {
  if (!slug || !payload?.kind) return;
  const record = pageRecord(slug, payload, identity);
  if (!record) return;
  pending.set(slug, record);
  clearTimeout(timers.get(slug));
  timers.set(slug, setTimeout(() => void write(slug), SETTLE_MS));
}

/// The page this browser last drew for a document, or null.
export async function lastPreview(slug) {
  if (!slug) return null;
  const database = await open();
  if (!database) return null;
  try {
    const store = database.transaction(PAGES, "readonly").objectStore(PAGES);
    const record = await result(store.get(slug));
    if (!record?.kind) return null;
    return record;
  } catch {
    return null;
  }
}

/// Writes a page that is waiting out its settling pause, for a document that
/// is being left. Nothing depends on this arriving.
export function flushPreview(slug) {
  if (!timers.has(slug)) return;
  clearTimeout(timers.get(slug));
  return write(slug);
}
