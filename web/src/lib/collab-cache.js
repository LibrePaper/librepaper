import * as Y from "yjs";
import { IndexeddbPersistence } from "y-indexeddb";

// A URL can be reused when a document is deleted and recreated (including
// `make deploy`'s examples). Its old CRDT must not become part of the new one.
export function cacheName(slug, createdAt = "") {
  return createdAt ? `komodoc-${slug}:${createdAt}` : `komodoc-${slug}`;
}

export function sharesHistory(left, right) {
  const known = Y.decodeStateVector(Y.encodeStateVector(left));
  return [...Y.decodeStateVector(Y.encodeStateVector(right))].some(
    ([client, clock]) => clock > 0 && (known.get(client) || 0) > 0,
  );
}

// Upgrade the previous, URL-only cache without dropping unsent edits. Import
// only if it shares CRDT history with the server's document. Keep the old
// database intact, including when it belongs to a deleted document.
export async function restoreLegacyCache(slug, doc) {
  const legacy = new Y.Doc();
  const store = new IndexeddbPersistence(cacheName(slug), legacy);
  try {
    await store.whenSynced;
    if (sharesHistory(doc, legacy)) Y.applyUpdate(doc, Y.encodeStateAsUpdate(legacy), "remote");
  } finally {
    await store.destroy();
    legacy.destroy();
  }
}
