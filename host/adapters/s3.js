// Any S3-compatible bucket: R2, AWS, MinIO, Backblaze, Ceph.
//
// Signed by hand rather than through an SDK. SigV4 is four HMACs over a
// canonical request plus a digest of the body, which is the code below; the
// alternative is pulling a dependency tree the size of the rest of this
// program into a program that has almost none. In JavaScript the HMACs are
// WebCrypto, which both runtimes have, so this adapter names no runtime and
// runs unchanged in a Worker.

import { BlobError } from '../core/blob.js'
import { amzStamps, nowUnix } from '../core/clock.js'
import { hex, sha256Hex, truncate, utf8 } from '../core/util.js'

export class S3Store {
  constructor(options, fetcher = globalThis.fetch.bind(globalThis)) {
    let prefix = options.prefix ?? ''
    if (prefix && !prefix.endsWith('/')) prefix += '/'
    this.endpoint = (options.endpoint ?? '').replace(/\/+$/, '')
    this.bucket = options.bucket ?? ''
    this.region = options.region ?? 'auto'
    this.prefix = prefix
    this.accessKey = options.access_key ?? options.accessKey ?? ''
    this.secretKey = options.secret_key ?? options.secretKey ?? ''
    /// Turns the conditional write off: see the storage options.
    this.singleWriter = Boolean(options.single_writer ?? options.singleWriter)
    this.fetch = fetcher
  }

  /// Puts a key under this deployment's prefix, so komodoc can share a bucket
  /// with whatever else the operator keeps there.
  scoped(key) {
    return `${this.prefix}${key}`
  }

  /// `escapePath` returns a path that already starts with a slash, so the
  /// bucket is joined to it directly rather than with one of its own.
  url(key) {
    return `${this.endpoint}/${this.bucket}${escapePath(this.scoped(key))}`
  }

  async write(key, body, contentType, conditions = []) {
    const headers = []
    if (contentType) headers.push(['content-type', contentType])
    for (const [name, value] of conditions) headers.push([name, value])
    const tag = `"${await sha256Hex(body)}"`
    const response = await this.send('PUT', this.url(key), headers, body)
    const status = response.status
    // 412 is the conditional write refusing: the object moved. 409 is what
    // some implementations answer to a lost If-None-Match race.
    if (status === 412 || status === 409) throw BlobError.conflict()
    if (status >= 300) throw await problem('PUT', key, response)
    // Some implementations do not return an ETag on PUT; the digest is the
    // same answer, and matches what a later GET will report.
    return etag(response) || tag
  }

  /// Signs a request and performs it.
  async send(method, target, headers, body) {
    let parsed
    try {
      parsed = new URL(target)
    } catch (error) {
      throw BlobError.other(error.message)
    }
    const host = parsed.host // includes the port when there is one
    const [stamp, day] = amzStamps(nowUnix())
    const payloadHash = await sha256Hex(body ?? new Uint8Array(0))

    const signed = new Map()
    signed.set('host', host)
    signed.set('x-amz-content-sha256', payloadHash)
    signed.set('x-amz-date', stamp)
    for (const [name, value] of headers) signed.set(name.toLowerCase(), String(value).trim())

    const names = [...signed.keys()].sort()
    const query = [...parsed.searchParams.entries()].map(([k, v]) => [k, v])
    const canonical = [
      method,
      escapePath(parsed.pathname),
      canonicalQuery(query),
      names.map((name) => `${name}:${signed.get(name)}\n`).join(''),
      names.join(';'),
      payloadHash,
    ].join('\n')
    const scope = `${day}/${this.region}/s3/aws4_request`
    const toSign = ['AWS4-HMAC-SHA256', stamp, scope, await sha256Hex(canonical)].join('\n')
    const signature = hex(await hmacSha256(await this.signingKey(day), utf8(toSign)))
    const authorization =
      `AWS4-HMAC-SHA256 Credential=${this.accessKey}/${scope}, ` +
      `SignedHeaders=${names.join(';')}, Signature=${signature}`

    const outgoing = new Headers()
    for (const name of names) {
      // `host` is the runtime's to set, and both refuse to let it be overridden.
      if (name !== 'host') outgoing.set(name, signed.get(name))
    }
    outgoing.set('authorization', authorization)

    try {
      return await this.fetch(target, {
        method,
        headers: outgoing,
        body: method === 'GET' || method === 'HEAD' ? undefined : body,
      })
    } catch (error) {
      throw BlobError.other(error?.message ?? String(error))
    }
  }

  async signingKey(day) {
    return await signingKeyFor(this.secretKey, day, this.region, 's3')
  }

  /// A URL that fetches one object and expires. It is what lets the bytes of a
  /// document go from the bucket to the reader without passing through the
  /// server. The URL is a bearer token for its lifetime, which is why the
  /// lifetime is short.
  ///
  /// Asynchronous where the Rust one was not, because WebCrypto's HMAC is; the
  /// interface's `presignedGet` is therefore awaited by every caller.
  async presignGet(key, lifetimeSeconds) {
    const [stamp, day] = amzStamps(nowUnix())
    const scope = `${day}/${this.region}/s3/aws4_request`
    const query = [
      ['X-Amz-Algorithm', 'AWS4-HMAC-SHA256'],
      ['X-Amz-Credential', `${this.accessKey}/${scope}`],
      ['X-Amz-Date', stamp],
      ['X-Amz-Expires', String(lifetimeSeconds)],
      ['X-Amz-SignedHeaders', 'host'],
    ]
    let target
    try {
      target = new URL(this.url(key))
    } catch {
      return null
    }
    const canonical = [
      'GET',
      escapePath(target.pathname),
      canonicalQuery(query),
      `host:${target.host}\n`,
      'host',
      'UNSIGNED-PAYLOAD',
    ].join('\n')
    const toSign = ['AWS4-HMAC-SHA256', stamp, scope, await sha256Hex(canonical)].join('\n')
    const signature = hex(await hmacSha256(await this.signingKey(day), utf8(toSign)))
    query.push(['X-Amz-Signature', signature])
    return `${target.origin}${target.pathname}?${canonicalQuery(query)}`
  }

  presignedGet(key, lifetimeSeconds) {
    return this.presignGet(key, lifetimeSeconds)
  }

  /// What a bucket turned out to support. A deployment must not discover on its
  /// first upload that its index cannot be written safely, so this runs at
  /// startup and is printed.
  async probe() {
    const SCRATCH = '.komodoc-probe'
    const report = { reachable: false, conditional_writes: false, why: '' }
    const scrub = async () => {
      try {
        await this.delete([SCRATCH])
      } catch {
        /* the probe object is not worth failing over */
      }
    }

    let first
    try {
      first = await this.write(SCRATCH, utf8('komodoc'), 'text/plain', [])
    } catch (error) {
      report.why = `the bucket could not be written to: ${error.message}`
      return report
    }
    report.reachable = true

    // A conditional write against the wrong version must be refused...
    const wrongVersion = '"0000000000000000000000000000000000000000"'
    try {
      await this.write(SCRATCH, utf8('no'), 'text/plain', [['If-Match', wrongVersion]])
      report.why =
        'a write conditional on the wrong version was accepted, so a lost update would go unnoticed.'
      await scrub()
      return report
    } catch (error) {
      if (error.kind !== 'conflict') {
        report.why = `a conditional write answered ${error.message} rather than refusing.`
        await scrub()
        return report
      }
    }

    // ...and so must a create-only write over something that exists.
    try {
      await this.write(SCRATCH, utf8('no'), 'text/plain', [['If-None-Match', '*']])
      report.why = 'a create-only write over an existing object was accepted.'
      await scrub()
      return report
    } catch (error) {
      if (error.kind !== 'conflict') {
        report.why = `a create-only write answered ${error.message} rather than refusing.`
        await scrub()
        return report
      }
    }

    // And the right version must be accepted, or nothing could ever be written.
    try {
      await this.write(SCRATCH, utf8('komodoc'), 'text/plain', [['If-Match', first]])
    } catch (error) {
      report.why = `a write conditional on the current version was refused: ${error.message}`
      await scrub()
      return report
    }

    await scrub()
    report.conditional_writes = true
    return report
  }

  /// The two things an operator has to set on their bucket, printed rather than
  /// described: a CORS rule, if documents are to be fetched by readers
  /// directly, and a credential scoped to this prefix rather than to the whole
  /// account.
  advice(origin) {
    return (
      `\nTo let readers fetch documents straight from ${this.bucket}, allow this origin:\n\n  ` +
      `[{"AllowedOrigins": ["${origin}"],\n    "AllowedMethods": ["GET", "HEAD"],\n    ` +
      `"AllowedHeaders": ["*"],\n    "ExposeHeaders": ["ETag"],\n    "MaxAgeSeconds": 3600}]\n\n` +
      `And a credential that can reach these keys and nothing else:\n\n  ` +
      `{"Version": "2012-10-17",\n   "Statement": [{"Effect": "Allow",\n     ` +
      `"Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket"],\n     ` +
      `"Resource": ["arn:aws:s3:::${this.bucket}", "arn:aws:s3:::${this.bucket}/${this.prefix}*"]}]}\n`
    )
  }

  /* ------------------------------------------------------- the interface */

  async get(key) {
    return (await this.getVersioned(key))[0]
  }

  async getVersioned(key) {
    const response = await this.send('GET', this.url(key), [], new Uint8Array(0))
    if (response.status === 404) throw BlobError.notFound()
    if (response.status !== 200) throw await problem('GET', key, response)
    const version = etag(response)
    const body = new Uint8Array(await response.arrayBuffer())
    return [body, version]
  }

  async put(key, body, contentType) {
    await this.write(key, body, contentType ?? '', [])
  }

  async delete(keys) {
    for (const key of keys) {
      const response = await this.send('DELETE', this.url(key), [], new Uint8Array(0))
      // An object that is not there is the outcome asked for.
      if (response.status >= 300 && response.status !== 404) {
        throw await problem('DELETE', key, response)
      }
    }
  }

  async list(prefix) {
    const found = []
    let token = ''
    for (;;) {
      const query = [
        ['list-type', '2'],
        ['prefix', this.scoped(prefix)],
      ]
      if (token) query.push(['continuation-token', token])
      const address = `${this.endpoint}/${this.bucket}?${canonicalQuery(query)}`
      const response = await this.send('GET', address, [], new Uint8Array(0))
      const body = await response.text()
      if (response.status !== 200) {
        throw BlobError.other(`listing ${prefix} failed (${response.status}): ${truncate(body, 200)}`)
      }
      const page = parseListing(body)
      for (const [key, size, version] of page.contents) {
        // Keys come back scoped; callers speak in unscoped keys.
        found.push({
          key: key.startsWith(this.prefix) ? key.slice(this.prefix.length) : key,
          size,
          version,
        })
      }
      if (!page.truncated || !page.nextToken) break
      token = page.nextToken
    }
    found.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0))
    return found
  }

  async swap(key, body, expect) {
    // With one writer asserted, the caller's lock is the coordination and the
    // bucket is asked for nothing it may not support.
    if (this.singleWriter) return await this.write(key, body, 'application/json', [])
    const condition = expect ? ['If-Match', expect] : ['If-None-Match', '*']
    return await this.write(key, body, 'application/json', [condition])
  }

  describe() {
    return `${this.endpoint}/${this.bucket}/${this.prefix}`
  }
}

/// How the probe reads, for the line `serve` prints at startup.
export function describeProbe(report, options) {
  let out = `  storage: ${options.endpoint}/${options.bucket}/${options.prefix}\n`
  if (!report.reachable) out += '  bucket: unreachable\n'
  else if (options.single_writer)
    out += '  bucket: single-writer, asserted; the index is written unconditionally\n'
  else if (report.conditional_writes)
    out += '  bucket: conditional writes work; the index is safe against a racing writer\n'
  else out += '  bucket: no conditional writes\n'
  return out
}

function etag(response) {
  return response.headers.get('etag') || ''
}

async function problem(method, key, response) {
  let body = ''
  try {
    body = await response.text()
  } catch {
    body = ''
  }
  return BlobError.other(
    `${method} ${key} failed (${response.status}): ${truncate(body.slice(0, 2048), 300)}`,
  )
}

/* ------------------------------------------------------------- listings */

/// ListObjectsV2's answer, in the fields this needs. Read with string searches
/// rather than an XML parser: the document is flat and the four elements are
/// unambiguous, and workerd has no DOMParser.
export function parseListing(body) {
  const listing = {
    truncated: element(body, 'IsTruncated') === 'true',
    nextToken: element(body, 'NextContinuationToken') ?? '',
    contents: [],
  }
  let rest = body
  for (;;) {
    const start = rest.indexOf('<Contents>')
    if (start < 0) break
    const after = rest.slice(start)
    const end = after.indexOf('</Contents>')
    if (end < 0) break
    const block = after.slice(0, end)
    const key = unescapeXml(element(block, 'Key') ?? '')
    const size = Number.parseInt(element(block, 'Size') ?? '0', 10) || 0
    const version = unescapeXml(element(block, 'ETag') ?? '')
    listing.contents.push([key, size, version])
    rest = after.slice(end)
  }
  return listing
}

function element(body, name) {
  const open = `<${name}>`
  const close = `</${name}>`
  const start = body.indexOf(open)
  if (start < 0) return null
  const from = start + open.length
  const end = body.indexOf(close, from)
  if (end < 0) return null
  return body.slice(from, end)
}

function unescapeXml(text) {
  return text
    .replaceAll('&quot;', '"')
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .replaceAll('&apos;', "'")
    .replaceAll('&amp;', '&')
}

/* -------------------------------------------------------------- signing */

/// The four-HMAC derivation, with the service named rather than assumed --
/// which is what lets a test check it against the vector AWS publishes, and
/// that vector is for a different service.
export async function signingKeyFor(secret, day, region, service) {
  let key = await hmacSha256(utf8(`AWS4${secret}`), utf8(day))
  key = await hmacSha256(key, utf8(region))
  key = await hmacSha256(key, utf8(service))
  return await hmacSha256(key, utf8('aws4_request'))
}

export async function hmacSha256(key, message) {
  const imported = await crypto.subtle.importKey(
    'raw',
    key,
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  )
  return new Uint8Array(await crypto.subtle.sign('HMAC', imported, message))
}

/// RFC 3986 unreserved characters stay; everything else is percent-encoded, a
/// space as %20 rather than a plus.
export function escape(value) {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  )
}

/// Encodes each segment of a path, keeping the slashes between them, and
/// returns it with a leading slash.
export function escapePath(path) {
  return `/${path.replace(/^\/+/, '').split('/').map(escape).join('/')}`
}

export function canonicalQuery(query) {
  const sorted = [...query].sort((a, b) => {
    if (a[0] !== b[0]) return a[0] < b[0] ? -1 : 1
    return a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0
  })
  return sorted.map(([key, value]) => `${escape(key)}=${escape(value)}`).join('&')
}
