// "Remove downloaded model" has to reach into
// Transformers.js's own cache rather than ask the library, which has no
// eviction API of its own. It stores every model file in the Cache Storage
// cache named "transformers-cache" (confirmed against
// node_modules/@huggingface/transformers/dist/transformers.js, `cacheKey`),
// keyed by the Hugging Face URL it fetched -- which contains the repo path,
// e.g. ".../onnx-community/whisper-small/resolve/<revision>/<file>". Deleting
// by that substring removes exactly one model's files and leaves the rest of
// the cache, and any other model, untouched.
const CACHE_NAME = "transformers-cache";

/// Deletes every cached request whose URL names `entry.repo`. Returns the
/// number of entries removed. Tolerates a browser with no Cache Storage and a
/// cache that was never created -- both mean there was nothing to remove.
export async function removeCachedModel(entry, { caches } = {}) {
  if (!caches || !entry?.repo) return 0;
  let cache;
  try {
    cache = await caches.open(CACHE_NAME);
  } catch {
    return 0;
  }
  if (!cache) return 0;
  const keys = (await cache.keys?.()) || [];
  const needle = `/${entry.repo}/`;
  let removed = 0;
  for (const request of keys) {
    if (!request?.url?.includes(needle)) continue;
    if (await cache.delete(request)) removed += 1;
  }
  return removed;
}
