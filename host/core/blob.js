// The bytes Komodoc keeps, addressed by key, wherever they live: a directory,
// a bucket somebody else pays for, or R2 through a Worker binding.
//
// Keys are one layout, whichever store holds them:
//
//     index.json
//     documents/<slug>/<sha>.html
//     sources/<slug>/<sha>
//     rooms/<slug>.json
//     rooms/<slug>.lock
//     examples/<slug>.json
//     session.key
//
// There is one interface, and three implementations of it under
// `host/adapters/`. This module is the interface, the errors and the keys: the
// part `core/` may import, since it names no runtime.

import { parseTimestamp, nowUnix, timestamp } from './clock.js'
import { sha256Hex } from './util.js'

/// The errors a store answers with. `NotFound` is an ordinary answer rather
/// than a failure -- a document with no source, a room nobody has commented in
/// -- and `Conflict` is a compare-and-swap that lost, after which the caller
/// re-reads and decides again.
export class BlobError extends Error {
  constructor(kind, message) {
    super(message ?? kind)
    this.name = 'BlobError'
    this.kind = kind
  }

  static notFound() {
    return new BlobError('not-found', 'no such object')
  }

  static conflict() {
    return new BlobError('conflict', 'the object was written by someone else')
  }

  static other(message) {
    return new BlobError('other', String(message))
  }
}

export function isNotFound(error) {
  return error instanceof BlobError && error.kind === 'not-found'
}

export function isConflict(error) {
  return error instanceof BlobError && error.kind === 'conflict'
}

/// `get`, returning null rather than throwing when there is nothing there. Most
/// callers want this; the ones that must tell "absent" from "broken" use `get`.
export async function getOrNull(blobs, key) {
  try {
    return await blobs.get(key)
  } catch (error) {
    if (isNotFound(error)) return null
    throw error
  }
}

/// How a store with no versions of its own supplies one: the digest of the
/// bytes. It has the property that matters -- it changes when the content
/// changes -- and it costs a hash of something already in memory.
export async function versionOf(body) {
  return `"${await sha256Hex(body)}"`
}

/* ----------------------------------------------------------------- keys */

/// The key layout, in one place, so a change to it is one change.
export const INDEX_KEY = 'index.json'
/// What cookies are signed with. Kept with everything else so a server that
/// holds no local state does not sign every reader out when it restarts.
export const SESSION_KEY_KEY = 'session.key'

export const documentKey = (slug, digest) => `documents/${slug}/${digest}.html`
export const documentPrefix = (slug) => `documents/${slug}/`

/// A source is stored under the digest of the version it was rendered into,
/// exactly as the HTML is, so publishing a new version never writes over the
/// source of the one the index still names. The index commits the digest, and
/// a source written for a version the index never named is unreachable --
/// which is the half-state to prefer.
export const sourceKey = (slug, digest) => `sources/${slug}/${digest}`
export const sourcePrefix = (slug) => `sources/${slug}/`

/// Where a source lived before it was versioned: one key for the document,
/// whatever version it was at. Read for documents published then, never
/// written.
export const legacySourceKey = (slug) => `sources/${slug}`
export const roomKey = (slug) => `rooms/${slug}.json`
export const roomLockKey = (slug) => `rooms/${slug}.lock`
export const examplesKey = (slug) => `examples/${slug}.json`

/// Removes everything komodoc wrote and nothing else. Seeding starts from
/// nothing, and on a bucket somebody else supplied, "nothing" means our keys
/// -- never the container, and never what else is in it.
export async function clearStorage(blobs) {
  for (const prefix of ['documents/', 'sources/', 'rooms/', 'examples/']) {
    let found
    try {
      found = await blobs.list(prefix)
    } catch {
      continue
    }
    const keys = found.map((object) => object.key)
    if (keys.length) {
      try {
        await blobs.delete(keys)
      } catch {
        /* a key that will not go is not a reason to stop clearing the rest */
      }
    }
  }
  try {
    await blobs.delete([INDEX_KEY])
  } catch {
    /* as above */
  }
}

/* ----------------------------------------------------------- room locks */

/// A room is the one piece of state a second server must not write behind the
/// first one's back: the in-memory copy is authoritative while anyone is
/// connected, so two servers on one bucket would each save over the other's
/// comments without either noticing.
///
/// The lock is one object holding who holds it and when they last said so. It
/// is not a distributed lock and does not pretend to be -- it is how a second
/// server finds out it is second, and refuses, instead of quietly
/// interleaving.
///
/// The port leaves it where it is and neither takes it nor requires it: the
/// executable is one process, and the Worker has the platform's guarantee.
/// These two functions exist so a bucket the Rust server wrote is left tidy.
export const LOCK_STALE_SECONDS = 5 * 60

/// Releases the locks a batch command took.
export async function releaseRoomLocks(blobs, slugs) {
  const keys = slugs.map(roomLockKey)
  if (!keys.length) return
  try {
    await blobs.delete(keys)
  } catch {
    /* a lock that will not go expires on its own */
  }
}

/// Claims the right to write this room, or says who already has it. An expired
/// lock is taken over: the holder is gone. Returns `{ mine, holder }`.
export async function takeRoomLock(blobs, slug, holder) {
  const key = roomLockKey(slug)
  let at = ''
  try {
    const [raw, version] = await blobs.getVersioned(key)
    let held = {}
    try {
      held = JSON.parse(new TextDecoder().decode(raw)) ?? {}
    } catch {
      held = {}
    }
    const taken = parseTimestamp(held.taken ?? '')
    const fresh = taken !== null && nowUnix() - taken < LOCK_STALE_SECONDS
    if (fresh && (held.holder ?? '') !== holder) return { mine: false, holder: held.holder ?? '' }
    at = version
  } catch (error) {
    if (!isNotFound(error)) {
      // Storage that cannot be read from is not storage that should be written
      // to blindly, but a lock is not worth refusing to serve over.
      return { mine: true, holder: '' }
    }
  }

  const body = new TextEncoder().encode(JSON.stringify({ holder, taken: timestamp() }))
  try {
    await blobs.swap(key, body, at)
    return { mine: true, holder }
  } catch (error) {
    if (isConflict(error)) return { mine: false, holder: 'another server' }
    // Storage without conditional writes; the assertion stands.
    return { mine: true, holder: '' }
  }
}
