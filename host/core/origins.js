// Documents are served from a different hostname than the reader, so an
// uploaded file is a stranger to the page framing it. The browser then refuses
// it any access to the reader's DOM or its session, which is what lets the
// document run its own scripts safely: charts, maps, anything.
//
// A different port would not do. Cookies ignore ports, so a document on
// another port could still make requests carrying the reader's session. It has
// to be a different host.
//
// The names are derived rather than configured: whatever host the reader is
// on, documents live on "docs." in front of it. On a real domain that is one
// DNS record and one certificate; in development, browsers resolve anything
// ending in .localhost by themselves, so it needs no setup at all.
//
// The port of `origins.rs`, line for line: these are the checks a mistake in
// would be a security bug, so they are not rewritten, only translated.

export const DOCS_PREFIX = 'docs.'

/// What a request tells about where it arrived: the host it was addressed to
/// and the scheme it arrived over, read once from the headers so every check
/// below sees the same answer.
export class Arrival {
  constructor(host, scheme) {
    this.host = host
    this.scheme = scheme
  }

  static fromHeaders(headers) {
    const host = header(headers, 'host') ?? ''
    const scheme = header(headers, 'x-forwarded-proto') === 'https' ? 'https' : 'http'
    return new Arrival(host, scheme)
  }

  /// Whether this request arrived on the document hostname.
  isDocsHost() {
    return this.host.toLowerCase().startsWith(DOCS_PREFIX)
  }

  /// The origin the reader frames documents from, and the only origin it
  /// accepts postMessage traffic from.
  docsOrigin() {
    return `${this.scheme}://${docsHost(this.host)}`
  }

  readerOrigin() {
    return `${this.scheme}://${readerHost(this.host)}`
  }

  isHttps() {
    return this.scheme === 'https'
  }

  callbackUrl() {
    return `${this.scheme}://${this.host}/auth/callback`
  }
}

/// One header, or null. Headers objects lower-case their names themselves, so
/// the caller need not.
export function header(headers, name) {
  const value = headers.get(name)
  return value === null || value === undefined ? null : value
}

/// Where documents for this deployment live.
export function docsHost(host) {
  return host.toLowerCase().startsWith(DOCS_PREFIX) ? host : `${DOCS_PREFIX}${host}`
}

/// The inverse: the reader that owns a document hostname.
export function readerHost(host) {
  return host.startsWith(DOCS_PREFIX) ? host.slice(DOCS_PREFIX.length) : host
}

/// Rule A, for a state-changing route a browser can reach with cookies
/// attached: docs.<host> is same-site with the reader, so SameSite cookies
/// alone do not stop a hostile document from posting here. A bearer token (the
/// CLI) skips all of this -- it is never attached to a request automatically,
/// so a hostile page cannot forge one. Otherwise all three must hold: any
/// Origin header sent must be this reader's own origin, any Sec-Fetch-Site
/// header must say the request was not cross-site, and a custom header must be
/// present, which a browser cannot attach to a cross-origin request without a
/// CORS preflight that is never granted.
export function crossSiteRefused(headers, arrival) {
  const authorization = header(headers, 'authorization')
  if (authorization && authorization.startsWith('Bearer ')) return false

  const origin = header(headers, 'origin')
  if (origin !== null && origin !== '' && origin !== arrival.readerOrigin()) return true

  const site = header(headers, 'sec-fetch-site')
  if (site !== null && site !== '' && site !== 'same-origin' && site !== 'none') return true

  const marker = header(headers, 'x-komodoc-client')
  return marker === null || marker === ''
}

/// Rule A's WebSocket variant: browsers always send Origin on a WebSocket
/// handshake and cannot be made to skip it or to attach a custom header, so
/// the custom-header check does not apply here -- an absent Origin is not
/// itself suspicious, but a foreign one is refused.
export function wsOriginRefused(headers, arrival) {
  const origin = header(headers, 'origin')
  if (origin === null) return false
  return origin !== '' && origin !== arrival.readerOrigin()
}

/// The JSON body every refusal under rule A answers with.
export function crossSiteRefusal() {
  return { error: 'cross-site request refused' }
}
