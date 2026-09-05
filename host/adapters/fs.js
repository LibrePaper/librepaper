// What `serve` has always done: a directory, one file per key. The process
// owns the directory, so its lock is the coordination and the
// compare-and-swap is bookkeeping rather than contention control.
//
// This is a Bun-side adapter, so it may import `node:fs`; `core/` may not.

import { mkdir, readFile, readdir, rename, rm, rmdir, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve, sep } from 'node:path'

import { BlobError, versionOf } from '../core/blob.js'
import { serialize } from './memory.js'

export class FsStore {
  constructor(dir) {
    this.dir = resolve(dir)
    this.swapping = Promise.resolve()
  }

  /// Maps a key to a file. Keys are slash-separated and come from this program,
  /// never from a request, but a key that escaped the directory would be a
  /// serious thing to get wrong, so it is checked rather than trusted.
  pathFor(key) {
    const cleaned = []
    for (const part of String(key).split('/')) {
      if (part === '' || part === '.') continue
      if (part === '..') {
        cleaned.pop()
        continue
      }
      cleaned.push(part)
    }
    if (!cleaned.length) throw BlobError.other('empty key')
    const name = join(this.dir, ...cleaned)
    // Belt and braces: after the walk above nothing can escape, but the check
    // costs nothing and the failure it catches is a serious one.
    if (name !== this.dir && !name.startsWith(this.dir + sep)) {
      throw BlobError.other('key escapes the data directory')
    }
    return name
  }

  async get(key) {
    return (await this.readVersioned(key))[0]
  }

  async getVersioned(key) {
    return await this.readVersioned(key)
  }

  async readVersioned(key) {
    let body
    try {
      body = new Uint8Array(await readFile(this.pathFor(key)))
    } catch (error) {
      throw translate(error)
    }
    return [body, await versionOf(body)]
  }

  async put(key, body) {
    await writeFileAtomically(this.pathFor(key), body)
  }

  async delete(keys) {
    for (const key of keys) {
      const name = this.pathFor(key)
      try {
        await rm(name)
      } catch (error) {
        if (error?.code !== 'ENOENT') throw translate(error)
      }
      // The directory a key lived in is part of the key, not a thing of its
      // own: an empty one left behind would show up in a listing as a document
      // that is not there.
      try {
        await rmdir(dirname(name))
      } catch {
        /* not empty, or not there; either way there is nothing to do */
      }
    }
  }

  async list(prefix) {
    const found = []
    await walk(this.dir, this.dir, prefix, found)
    found.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0))
    return found
  }

  async swap(key, body, expect) {
    return await serialize(this, async () => {
      let current = ''
      try {
        current = (await this.readVersioned(key))[1]
      } catch (error) {
        if (error?.kind !== 'not-found') throw error
      }
      if (current !== expect) throw BlobError.conflict()
      await this.put(key, body)
      return await versionOf(body)
    })
  }

  /// Says where these bytes are, for the line `serve` prints at startup. An
  /// operator should never have to guess which directory they are writing to.
  describe() {
    return this.dir
  }

  /// A directory cannot mint a URL the reader's browser can fetch.
  presignedGet() {
    return null
  }
}

function translate(error) {
  if (error instanceof BlobError) return error
  if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return BlobError.notFound()
  return BlobError.other(error?.message ?? String(error))
}

async function walk(root, dir, prefix, found) {
  let entries
  try {
    entries = await readdir(dir, { withFileTypes: true })
  } catch (error) {
    // A directory that vanished mid-walk is not a listing failure.
    if (error?.code === 'ENOENT') return
    throw translate(error)
  }
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) {
      await walk(root, path, prefix, found)
      continue
    }
    const key = path.slice(root.length + 1).split(sep).join('/')
    if (!key.startsWith(prefix)) continue
    let info
    try {
      info = await stat(path)
    } catch {
      continue
    }
    found.push({ key, size: info.size, version: '' })
  }
}

/// Leaves either the old bytes or the new ones, never a half-written file: a
/// crash mid-write must not turn the index into something that no longer
/// parses.
export async function writeFileAtomically(name, body) {
  await mkdir(dirname(name), { recursive: true })
  const temporary = `${name}.tmp`
  await writeFile(temporary, body)
  await rename(temporary, name)
}
