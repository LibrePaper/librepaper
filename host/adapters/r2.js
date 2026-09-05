// R2 through a Worker binding: the smallest of the three adapters, because the
// platform supplies conditional writes and listing directly. The old Worker's
// `blobs(env)` was this.

import { BlobError } from '../core/blob.js'

export class R2Store {
  constructor(bucket, prefix = '') {
    this.bucket = bucket
    this.prefix = prefix
  }

  full(key) {
    return `${this.prefix}${key}`
  }

  async get(key) {
    const object = await this.bucket.get(this.full(key))
    if (!object) throw BlobError.notFound()
    return new Uint8Array(await object.arrayBuffer())
  }

  async getVersioned(key) {
    const object = await this.bucket.get(this.full(key))
    if (!object) throw BlobError.notFound()
    return [new Uint8Array(await object.arrayBuffer()), quoted(object.etag)]
  }

  async put(key, body, contentType) {
    await this.bucket.put(this.full(key), body, {
      httpMetadata: contentType ? { contentType } : undefined,
    })
  }

  async delete(keys) {
    if (!keys.length) return
    await this.bucket.delete(keys.map((key) => this.full(key)))
  }

  async list(prefix) {
    const found = []
    let cursor
    for (;;) {
      const page = await this.bucket.list({ prefix: this.full(prefix), cursor, limit: 1000 })
      for (const object of page.objects) {
        found.push({
          key: object.key.slice(this.prefix.length),
          size: object.size,
          version: quoted(object.etag),
        })
      }
      if (!page.truncated) break
      cursor = page.cursor
    }
    found.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0))
    return found
  }

  /// The platform's own compare-and-swap. An empty expected version means
  /// "only if it does not exist", which R2 spells `etagMatches: undefined` plus
  /// a null check -- there is no "if-none-match: *" on `put`, so the absent
  /// case is expressed by asking for a nonexistent etag to match nothing.
  async swap(key, body, expect) {
    const name = this.full(key)
    const onlyIf = expect ? { etagMatches: unquoted(expect) } : { etagDoesNotMatch: '*' }
    const written = await this.bucket.put(name, body, { onlyIf })
    if (written === null) throw BlobError.conflict()
    return quoted(written.etag)
  }

  describe() {
    return `r2 bucket${this.prefix ? ` (${this.prefix})` : ''}`
  }

  /// A binding cannot mint a URL; a public bucket hostname is configuration,
  /// not something this adapter knows.
  presignedGet() {
    return null
  }
}

/// R2 reports etags bare; the interface's versions are quoted, as S3's are, so
/// a version read from one store and compared against another means the same
/// thing.
function quoted(etag) {
  if (!etag) return ''
  return etag.startsWith('"') ? etag : `"${etag}"`
}

function unquoted(etag) {
  return etag.replace(/^"|"$/g, '')
}
