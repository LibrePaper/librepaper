// A URL can be reused when a document is deleted and recreated (including
// `make deploy`'s examples). Its old CRDT must not become part of the new one.
export function cacheName(slug, createdAt = "") {
  return createdAt ? `librepaper-${slug}:${createdAt}` : `librepaper-${slug}`;
}
