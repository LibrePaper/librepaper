// Small things several modules share. The port of `util.rs`, minus the
// terminal helpers, which belong to the command line and live under `bun/`.

/// The first value that is not empty once unquoted.
export function firstOf(values) {
  for (const value of values) {
    const cleaned = unquote(value ?? '')
    if (cleaned) return cleaned
  }
  return ''
}

/// Drops surrounding quotes. A .env read by make keeps them, unlike a shell,
/// and a client id wearing quotation marks is one GitHub has never heard of.
/// Every other .env convention allows them, so accept them here.
export function unquote(value) {
  const trimmed = String(value ?? '').trim()
  if (trimmed.length >= 2) {
    const first = trimmed[0]
    const last = trimmed[trimmed.length - 1]
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return trimmed.slice(1, -1).trim()
    }
  }
  return trimmed
}

/// Strips control characters and trims to a length in characters, matching
/// what every backend has always stored. "Characters" means what Rust's
/// `chars()` meant: Unicode scalar values, which is what iterating a string
/// with for..of gives.
export function clean(value, limit) {
  const kept = []
  for (const character of String(value ?? '')) {
    const code = character.codePointAt(0)
    const control =
      code < 0x09 || (code >= 0x0b && code <= 0x0c) || (code >= 0x0e && code <= 0x1f) || code === 0x7f
    if (control) continue
    kept.push(character)
    if (kept.length >= limit) break
  }
  return kept.join('')
}

/// A random UUID v4.
export function newId() {
  return crypto.randomUUID()
}

/// Random bytes from the platform CSPRNG. Both runtimes have WebCrypto, so
/// there is one implementation.
export function randomBytes(count) {
  const bytes = new Uint8Array(count)
  crypto.getRandomValues(bytes)
  return bytes
}

export function hex(bytes) {
  let out = ''
  for (const byte of bytes) out += byte.toString(16).padStart(2, '0')
  return out
}

export function unhex(text) {
  const clean = String(text ?? '')
  if (clean.length % 2 !== 0) return new Uint8Array(0)
  const out = new Uint8Array(clean.length / 2)
  for (let i = 0; i < out.length; i += 1) {
    const byte = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16)
    if (Number.isNaN(byte)) return new Uint8Array(0)
    out[i] = byte
  }
  return out
}

const encoder = new TextEncoder()
const decoder = new TextDecoder()

export function utf8(text) {
  return encoder.encode(text)
}

export function fromUtf8(bytes) {
  return decoder.decode(bytes)
}

/// Truncates to a length in bytes without splitting a character, which is what
/// the Rust `truncate` did by walking back to a char boundary.
export function truncate(text, limit) {
  const bytes = utf8(text)
  if (bytes.length <= limit) return text
  return new TextDecoder('utf-8', { fatal: false }).decode(bytes.slice(0, limit)).replace(/�$/, '')
}

/// Base64 for bytes, and back. Both runtimes have atob/btoa; neither has a
/// byte-oriented one, so this is the usual pair of loops.
export function base64(bytes) {
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}

export function unbase64(text) {
  const binary = atob(text)
  const out = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i)
  return out
}

/// SHA-256, hex, of bytes or a string. Used for document digests and for the
/// version a store with no versions of its own supplies.
export async function sha256Hex(input) {
  const bytes = typeof input === 'string' ? utf8(input) : input
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  return hex(new Uint8Array(digest))
}

/// A comparison whose duration does not depend on where the first difference
/// is. Signatures are compared with it and nothing else.
export function timingSafeEqual(a, b) {
  const left = typeof a === 'string' ? utf8(a) : a
  const right = typeof b === 'string' ? utf8(b) : b
  if (left.length !== right.length) return false
  let difference = 0
  for (let i = 0; i < left.length; i += 1) difference |= left[i] ^ right[i]
  return difference === 0
}
