// The reader's initial metadata load. It owns the two requests it starts and
// invalidates their callbacks when the reader is torn down or restarted.
import { keyHeaders, me as defaultWhoami } from "../api.js";

const DOCUMENT_CACHE = "librepaper";

// Rendering results used to be kept in a document-scoped IndexedDB outbox
// and, in an earlier client, under the shared immutable-asset cache. The
// latter must be pruned entry by entry: deleting the whole cache would also
// delete source input assets that the current client still uses.
function isLegacyGeneratedOutput(request) {
  const value = typeof request === "string" ? request : request?.url;
  if (!value) return false;
  let path;
  try { path = new URL(value, globalThis.location?.href || "http://localhost/").pathname; }
  catch { return false; }
  return /^\/api\/documents\/[^/]+\/(?:renderings?(?:\/|$)|quarto\/bundles(?:\/|$)|results?(?:\/|$))/i.test(path);
}

async function clearLegacyCachedOutputs(cacheStorage) {
  if (!cacheStorage?.open || !cacheStorage?.keys) return false;
  try {
    if (!(await cacheStorage.keys()).includes(DOCUMENT_CACHE)) return false;
    const store = await cacheStorage.open(DOCUMENT_CACHE);
    const keys = await store.keys();
    await Promise.all(keys.filter(isLegacyGeneratedOutput).map((request) => store.delete(request)));
    return true;
  } catch {
    return false;
  }
}

export function clearLegacyRenderingCaches({
  indexedDB: database = globalThis.indexedDB,
  caches: cacheStorage = globalThis.caches,
} = {}) {
  const databaseCleanup = database?.deleteDatabase
    ? new Promise((resolve) => {
      const request = database.deleteDatabase("librepaper-quarto-outbox");
      request.onsuccess = () => resolve(true);
      request.onerror = request.onblocked = () => resolve(false);
    })
    : Promise.resolve(false);
  return Promise.all([databaseCleanup, clearLegacyCachedOutputs(cacheStorage)])
    .then(([databaseCleared, cacheCleared]) => databaseCleared || cacheCleared);
}

export function createReaderBoot({
  slug,
  key = "",
  fetcher = globalThis.fetch,
  whoami = defaultWhoami,
  onIdentity = () => {},
  onDocument = () => {},
  onError = () => {},
}) {
  let generation = 0;
  let disposed = false;
  const controllers = new Set();

  async function requestDocument(current) {
    const controller = new AbortController();
    controllers.add(controller);
    try {
      const response = await fetcher(`/api/documents/${slug}`, {
        headers: keyHeaders(key),
        signal: controller.signal,
      });
      if (!response.ok) throw new Error("not found");
      const document_ = await response.json();
      if (!disposed && current === generation) onDocument(document_);
    } catch (error) {
      if (error?.name !== "AbortError" && !disposed && current === generation) onError(error);
    } finally {
      controllers.delete(controller);
    }
  }

  async function start() {
    if (disposed) return;
    const current = ++generation;
    // Identity is deliberately independent of document access. A failed
    // identity endpoint still leaves the document request useful, while a
    // late identity response can never rename a replaced reader session.
    Promise.resolve().then(() => whoami()).then((who) => {
      if (!disposed && current === generation) onIdentity(who || {});
    }).catch(() => {
      if (!disposed && current === generation) onIdentity({});
    });
    return requestDocument(current);
  }

  function dispose() {
    disposed = true;
    generation += 1;
    for (const controller of controllers) controller.abort();
    controllers.clear();
  }

  return { start, dispose };
}
