// The document's figures, as this browser holds them.
//
// A figure is not in the shared document: what travels there is a path and the
// digest of the bytes at it. The bytes come from the store, once, and stay --
// content-addressed, so a figure fetched for one version of a document is the
// same figure for every later one, and a re-render on a keystroke reads it
// from memory rather than from anywhere.
//
// Two forms are needed, because the two renderers want different things. Typst
// takes the bytes and writes the image into the page itself. Markdown produces
// HTML a browser will fetch from, so it takes a `blob:` URL -- never the route
// the bytes came from, which on a private document carries a credential and
// would put it inside a rendered page.

const bytes = new Map(); // digest -> Uint8Array
const urls = new Map(); // digest -> blob: URL
const inFlight = new Map(); // digest -> Promise
const scopes = new Map();
let generation = 0;
let nextScope = 0;
function scopedKey(slug, sha, headers) {
  const entries = headers instanceof Headers ? [...headers.entries()] : Object.entries(headers || {});
  const fingerprint = JSON.stringify([slug, entries.sort(([a], [b]) => a.localeCompare(b))]);
  if (!scopes.has(fingerprint)) scopes.set(fingerprint, ++nextScope);
  return `${scopes.get(fingerprint)}:${sha}`;
}

/// The bytes of a figure, fetched once per browser and kept.
async function fetchOne(slug, sha, headers) {
  const key = scopedKey(slug, sha, headers);
  const captured = generation;
  if (bytes.has(key)) return bytes.get(key);
  if (inFlight.has(key)) return inFlight.get(key);
  const url = `/api/documents/${slug}/assets/${sha}`;
  const wanted = (async () => {
    const response = await fetch(url, { credentials: "same-origin", headers, cache: "no-store" });
    if (!response.ok) throw new Error(`could not fetch a figure (${response.status})`);
    const body = new Uint8Array(await response.arrayBuffer());
    if (generation !== captured) throw new Error("Asset authorization changed");
    if (/^[0-9a-f]{64}$/.test(sha)) {
      const actual = [...new Uint8Array(await crypto.subtle.digest("SHA-256", body))].map((byte) => byte.toString(16).padStart(2, "0")).join("");
      if (actual !== sha) throw new Error("Captured asset failed its integrity check");
    }
    if (generation !== captured) throw new Error("Asset authorization changed");
    bytes.set(key, body);
    return body;
  })();
  inFlight.set(key, wanted);
  try {
    return await wanted;
  } finally {
    if (inFlight.get(key) === wanted) inFlight.delete(key);
  }
}

/// What a `blob:` URL for these bytes should claim to be. The store answers
/// every figure as octet-stream -- it knows a digest and not a filename -- so
/// the type is worked out here, from the name the document gave it.
function typeOf(path) {
  const lower = path.toLowerCase();
  if (lower.endsWith(".png")) return "image/png";
  if (lower.endsWith(".jpg") || lower.endsWith(".jpeg")) return "image/jpeg";
  if (lower.endsWith(".gif")) return "image/gif";
  if (lower.endsWith(".webp")) return "image/webp";
  if (lower.endsWith(".svg")) return "image/svg+xml";
  if (lower.endsWith(".pdf")) return "application/pdf";
  return "application/octet-stream";
}

/// Fetches every figure a document names and returns them in both forms.
/// `digests` is the path-to-digest map the shared document carries.
///
/// A figure that cannot be fetched is left out rather than failing the render:
/// the rest of the document is still worth showing, and the image is broken in
/// the page, which is what a missing figure looks like everywhere else.
export async function gather(slug, digests, headers = {}, { strict = false } = {}) {
  const wanted = Object.entries(digests || {});
  const got = await Promise.all(
    wanted.map(async ([path, sha]) => {
      try {
        const key = scopedKey(slug, sha, headers);
        const captured = generation;
        const body = await fetchOne(slug, sha, headers);
        if (generation !== captured) throw new Error("Asset authorization changed");
        if (!urls.has(key)) {
          const objectUrl = URL.createObjectURL(new Blob([body], { type: typeOf(path) }));
          // Blob URLs are recreated on every page load. Keep the immutable
          // store identity in the fragment, which is ignored by the resource
          // fetch but available to the injected document agent.
          urls.set(key, `${objectUrl}#librepaper-asset=${encodeURIComponent(sha)}`);
        }
        return [path, body, urls.get(key)];
      } catch {
        return null;
      }
    }),
  );
  const assets = {};
  const where = {};
  const missing = [];
  for (const one of got) {
    if (!one) continue;
    const [path, body, url] = one;
    assets[path] = body;
    where[path] = url;
  }
  for (const [path] of wanted) {
    if (!Object.prototype.hasOwnProperty.call(assets, path)) missing.push(path);
  }
  if (strict && missing.length) {
    const error = new Error(`could not fetch figure${missing.length === 1 ? "" : "s"}: ${missing.join(", ")}`);
    error.missing = missing;
    throw error;
  }
  return { assets, urls: where, missing };
}

/// Whether every figure a document names is already here, which is what says a
/// render can go ahead without waiting. A render that went ahead without them
/// would produce a page with holes in it and then be replaced a moment later,
/// which reads as a flicker.
export function ready(digests, slug = "", headers = {}) {
  return Object.values(digests || {}).every((sha) => bytes.has(scopedKey(slug, sha, headers)));
}

/// Forgets everything, for a page leaving a document. The blob URLs are
/// revoked: each one holds its bytes alive in the browser until it is.
export function release() {
  generation++;
  for (const url of urls.values()) URL.revokeObjectURL(url.split("#", 1)[0]);
  urls.clear();
  bytes.clear();
  inFlight.clear();
  scopes.clear();
}
