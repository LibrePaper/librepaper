// Timestamps, the one shape both the index and the room use: RFC 3339 in UTC
// to the second, "2026-09-04T12:00:00Z".
//
// The clock is a module-level hook rather than a direct call to Date.now, so
// the tests can move time forward without sleeping and `set_today` can hand
// the engine the same day the host believes in.

let source = () => Math.floor(Date.now() / 1000)

/// Replaces the clock. The tests use it; nothing else should.
export function setClock(fn) {
  source = fn
}

export function nowUnix() {
  return source()
}

export function timestamp() {
  return formatUnix(nowUnix())
}

export function formatUnix(unix) {
  if (!Number.isFinite(unix)) return ''
  const at = new Date(unix * 1000)
  if (Number.isNaN(at.getTime())) return ''
  return at.toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/// An RFC 3339 timestamp as seconds since the epoch, or null when it is not
/// one. Deliberately strict: `Date.parse` accepts a great deal that is not
/// RFC 3339, and a lock or a retention sweep reading a loose date as a valid
/// one is worse than reading it as absent.
export function parseTimestamp(value) {
  if (typeof value !== 'string') return null
  if (!/^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/.test(value)) {
    return null
  }
  const at = Date.parse(value)
  return Number.isNaN(at) ? null : Math.floor(at / 1000)
}

/// The date and time SigV4 wants: "20260904T120000Z" and "20260904".
export function amzStamps(unix) {
  const iso = formatUnix(unix)
  const stamp = iso.replace(/[-:]/g, '')
  return [stamp, stamp.slice(0, 8)]
}

/// The day the engine is told about, as "YYYY-MM-DD".
export function today() {
  return formatUnix(nowUnix()).slice(0, 10)
}
