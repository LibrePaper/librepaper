// Cache Storage, opened once, for the bytes that never change.
//
// Two callers want the same thing for different reasons. A distribution's
// files are tens of megabytes and are fetched once per browser rather than
// once per document; a document's assets are content-addressed and are
// fetched once rather than once per render. Both are URLs whose bytes cannot
// change, because the digest is in the URL, so both are cached forever and
// neither is ever invalidated: a new version is a new URL.
//
// The key is the URL and only the URL. A private document's asset is fetched
// with a short-lived token, and a token in the key would mean that rotating
// it permanently missed the figures it had already cached. So the request may
// carry whatever it needs and the entry is stored under the bare URL.
//
// Nothing here throws. A browser with no Cache Storage -- a private window, a
// context without `caches` -- still fetches; it just fetches again next time.

const CACHE = "librepaper";

let opened = null;

function open() {
  if (opened) return opened;
  opened = Promise.resolve()
    .then(() => (globalThis.caches ? caches.open(CACHE) : null))
    .catch(() => null);
  return opened;
}

/// The response for a URL whose bytes are immutable: from the cache if it is
/// there, from the network otherwise, and into the cache on the way past.
/// `init` is handed to `fetch` and never to the key.
export async function cached(url, init) {
  const store = await open();
  if (!store) return fetch(url, init);
  const hit = await store.match(url).catch(() => null);
  if (hit) return hit;
  const response = await fetch(url, init);
  // Only a success is kept. A 404 cached forever would be a document that
  // never renders again, which is a worse failure than a slow one.
  if (response.ok) {
    store.put(url, response.clone()).catch(() => {
      /* a full or refused quota is not a reason to fail the render */
    });
  }
  return response;
}

let asked = null;

/// Asks, once, that this origin's storage not be evicted, and tolerates a
/// refusal. A browser that says no still works; the distribution may simply
/// have to be fetched again some day, which the card says.
export function persist() {
  if (asked) return asked;
  asked = Promise.resolve()
    .then(() => navigator?.storage?.persist?.() ?? false)
    .catch(() => false);
  return asked;
}
