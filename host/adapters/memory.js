// A blob store in a Map. The tests run `core/` against it, and it is the
// reference the other three are checked against: the same suite runs over all
// of them, so a divergence is a failing test rather than a surprise in
// production.

import { BlobError, versionOf } from '../core/blob.js'

export class MemoryStore {
  constructor() {
    this.objects = new Map()
    // One writer at a time, so a swap cannot be overtaken between reading a
    // version and writing the next one. JavaScript is single-threaded, but a
    // swap awaits between the two, and an await is where another one gets in.
    this.swapping = Promise.resolve()
  }

  async get(key) {
    const found = this.objects.get(key)
    if (!found) throw BlobError.notFound()
    return found.body.slice()
  }

  async getVersioned(key) {
    const found = this.objects.get(key)
    if (!found) throw BlobError.notFound()
    return [found.body.slice(), found.version]
  }

  async put(key, body, contentType) {
    this.objects.set(key, {
      body: body.slice(),
      version: await versionOf(body),
      contentType: contentType ?? 'application/octet-stream',
    })
  }

  async delete(keys) {
    for (const key of keys) this.objects.delete(key)
  }

  async list(prefix) {
    const found = []
    for (const [key, object] of this.objects) {
      if (!key.startsWith(prefix)) continue
      found.push({ key, size: object.body.length, version: object.version })
    }
    found.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0))
    return found
  }

  async swap(key, body, expect) {
    return await serialize(this, async () => {
      const current = this.objects.get(key)?.version ?? ''
      if (current !== expect) throw BlobError.conflict()
      await this.put(key, body, 'application/octet-stream')
      return this.objects.get(key).version
    })
  }

  describe() {
    return 'memory'
  }

  presignedGet() {
    return null
  }
}

/// Runs `work` after whatever is already queued on this store, so two swaps
/// cannot interleave across their awaits.
export function serialize(store, work) {
  const next = store.swapping.then(work, work)
  // The queue must not reject, or every later swap inherits the rejection.
  store.swapping = next.then(
    () => {},
    () => {},
  )
  return next
}
