// Routing, uploads, and serving documents. The Room class comes from
// room.js; the two are concatenated into one module at build time.
export { Room } from "./room.js";

// The reader shell -- HTML, CSS and the client modules -- is injected here at
// build time from src/shell, keyed by request path.
const SHELL = __SHELL__;

const SLUG = new RegExp(CONFIG.slug_pattern);
const SHA = /^[0-9a-f]{64}$/;
const MAX_HTML = CONFIG.max_html;
const VISITOR_COOKIE = "komodoc_visitor";
// A colon cannot appear in a GitHub login, so a browser's key never collides
// with an account's.
const VISITOR_PREFIX = "visitor:";

const json = (data, status = 200, headers = {}) =>
  new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", ...headers },
  });

async function sha256(input) {
  const bytes = typeof input === "string" ? new TextEncoder().encode(input) : input;
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// --- identity ---------------------------------------------------------------
// The same rules serve enforces, in the runtime the Worker has. A policy is
// "anyone" (no sign-in), "any" (any GitHub account), or a list of logins.

function parsePolicy(value) {
  const trimmed = String(value || "").trim().toLowerCase();
  if (trimmed === "anyone" || trimmed === "public") return { public: true, logins: [] };
  // "anygithub" is accepted because parsePolicy in auth.go accepts it: a
  // deployment configured with it would otherwise be read here as a list
  // holding one impossible login, and let nobody in at all.
  if (trimmed === "any" || trimmed === "*" || trimmed === "anygithub") return { any: true, logins: [] };
  return { logins: trimmed.split(",").map((entry) => entry.trim()).filter(Boolean) };
}

function policyAllows(policy, login) {
  if (policy.public) return true;
  if (!login) return false;
  if (policy.any) return true;
  return policy.logins.some((allowed) => allowed === login.toLowerCase());
}

function describePolicy(policy) {
  if (policy.public) return "anyone";
  if (policy.any) return "any GitHub account";
  if (!policy.logins.length) return "nobody (unconfigured)";
  return "@" + policy.logins.join(", @");
}

const base64url = (bytes) =>
  btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

async function signPayload(key, payload) {
  const material = await crypto.subtle.importKey(
    "raw", new TextEncoder().encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return base64url(await crypto.subtle.sign("HMAC", material, new TextEncoder().encode(payload)));
}

// A session is "<login|expiry>.<signature>". Nothing is stored: the signature
// is what makes it trustworthy.
async function makeSession(env, login, expiry) {
  const payload = base64url(new TextEncoder().encode(`${login}|${expiry}`));
  return `${payload}.${await signPayload(env.KOMODOC_SESSION_KEY || "", payload)}`;
}

async function readSession(env, cookie) {
  const [payload, signature] = String(cookie || "").split(".");
  if (!payload || !signature) return "";
  if ((await signPayload(env.KOMODOC_SESSION_KEY || "", payload)) !== signature) return "";
  const decoded = new TextDecoder().decode(
    Uint8Array.from(atob(payload.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0)));
  const [login, stamp] = decoded.split("|");
  if (!login || Number(stamp) * 1000 < Date.now()) return "";
  return login;
}

function cookieValue(request, name) {
  const header = request.headers.get("cookie") || "";
  for (const part of header.split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=");
  }
  return "";
}

// signVisitor and readVisitor sign the visitor cookie the same way sessions
// are signed, over the same key: a client that could set komodoc_visitor to
// anything would manufacture as many owners -- and, through documentRoom, as
// many Durable Objects -- as it liked.
async function signVisitor(env, token) {
  return `${token}.${await signPayload(env.KOMODOC_SESSION_KEY || "", token)}`;
}

async function readVisitor(env, cookie) {
  const [token, signature] = String(cookie || "").split(".");
  if (!token || !signature) return "";
  if ((await signPayload(env.KOMODOC_SESSION_KEY || "", token)) !== signature) return "";
  return token;
}

// bearerCache remembers who a GitHub token belongs to for five minutes, so a
// CLI session that calls the API repeatedly does not cost a GitHub request
// per call. Isolates are short-lived, so this stays small on its own; the
// cap below is a backstop against an isolate that somehow lives long enough
// to see many distinct tokens.
const bearerCache = new Map();
const BEARER_TTL_MS = 5 * 60 * 1000;
const BEARER_CACHE_LIMIT = 1000;

async function githubLogin(token) {
  const key = await sha256(token);
  const cached = bearerCache.get(key);
  if (cached && cached.expires > Date.now()) return cached.login;

  const response = await fetch("https://api.github.com/user", {
    headers: {
      authorization: `Bearer ${token}`,
      accept: "application/vnd.github+json",
      "user-agent": "komodoc",
    },
  });
  if (!response.ok) return "";
  const login = (await response.json()).login || "";
  if (bearerCache.size >= BEARER_CACHE_LIMIT) bearerCache.clear();
  bearerCache.set(key, { login, expires: Date.now() + BEARER_TTL_MS });
  return login;
}

// whoami identifies a caller: a browser by its session cookie, the CLI by the
// GitHub token it sends as a bearer.
async function whoami(request, env) {
  const header = request.headers.get("authorization") || "";
  if (header.startsWith("Bearer ")) return githubLogin(header.slice(7));
  return readSession(env, cookieValue(request, "komodoc_session"));
}

// owner is the key a caller's uploads belong to. A signed-in caller is their
// GitHub login. Where publishing needs no account there is still someone on
// the other end, so an anonymous caller is named by the visitor cookie the
// shell handed their browser: not an identity, but enough that one visitor's
// uploads are not another's to list, replace or delete.
//
// A caller with neither -- the CLI publishing to a deployment open to
// everyone -- owns nothing, and their uploads stay shared.
async function ownerKey(request, env, login) {
  if (login) return login;
  const visitor = await readVisitor(env, cookieValue(request, VISITOR_COOKIE));
  return visitor ? VISITOR_PREFIX + visitor : "";
}

// publisher returns { owner, login } when allowed, or { refusal } to send
// back. login is returned alongside owner so a caller that needs it too --
// the example gate, in particular -- does not have to verify identity a
// second time.
async function publisher(request, env) {
  const login = (await whoami(request, env)).toLowerCase();
  const policy = parsePolicy(env.KOMODOC_PUBLISHERS);
  if (policyAllows(policy, login)) return { owner: await ownerKey(request, env, login), login };
  if (!login) return { refusal: json({ error: "sign in with GitHub to publish" }, 401) };
  return {
    refusal: json(
      { error: `@${login} may not publish here; this deployment allows ${describePolicy(policy)}` },
      403,
    ),
  };
}

// Reserved examples are installed only by the accounts a deployment names
// with --examples; the policy is never "any" or "anyone" (deploy refuses to
// configure it that way), so a non-empty login list is what "enabled" means.
function examplesEnabled(env) {
  return parsePolicy(env.KOMODOC_EXAMPLES).logins.length > 0;
}

// A document with no publisher belongs to no one in particular and stays
// shared, which is how every document behaved before ownership was recorded.
const ownedBy = (entry, owner) => !entry.publisher || entry.publisher === owner;

// Reserved examples are everyone's; an owner otherwise sees the documents that
// predate ownership and their own uploads.
function visibleTo(entries, owner) {
  return entries.filter((entry) => entry.example || ownedBy(entry, owner));
}

function slugify(value) {
  const slug = String(value || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, CONFIG.slug_max);
  return slug || null;
}

function randomSuffix() {
  const alphabet = CONFIG.suffix_alphabet;
  return [...crypto.getRandomValues(new Uint8Array(CONFIG.suffix_length))]
    .map((byte) => alphabet[byte % alphabet.length])
    .join("");
}

async function readIndex(env) {
  const object = await env.DOCS.get("index.json");
  if (!object) return { entries: {}, etag: null };
  return { entries: await object.json(), etag: object.etag };
}

// index.json is the only object with more than one writer, and only ever on
// upload. Compare-and-swap on the ETag rather than locking.
async function updateIndex(env, mutate) {
  for (let attempt = 0; attempt < 4; attempt++) {
    const { entries, etag } = await readIndex(env);
    mutate(entries);
    const condition = etag ? { etagMatches: etag } : { etagDoesNotMatch: "*" };
    const written = await env.DOCS.put("index.json", JSON.stringify(entries), {
      onlyIf: condition,
      httpMetadata: { contentType: "application/json", cacheControl: "no-cache" },
    });
    if (written) return entries;
  }
  throw new Error("index.json is contended; try again");
}

// localPath is where a sign-in may return to: somewhere on this site, and
// nowhere else. A value like "//elsewhere.example" starts with a slash but is
// read by browsers as an absolute URL, which would make the callback an open
// redirect, so the path is parsed and required to carry no scheme or host.
function localPath(next) {
  if (!next || !next.startsWith("/") || next.startsWith("//")) return "/";
  try {
    const parsed = new URL(next, "https://komodoc.invalid");
    if (parsed.origin !== "https://komodoc.invalid") return "/";
    return parsed.pathname + parsed.search + parsed.hash;
  } catch {
    return "/";
  }
}

function room(env, slug) {
  return env.ROOM.get(env.ROOM.idFromName(slug));
}

// The document runs on its own origin, with nothing of the reader's to reach
// for, so it may run its own scripts. What it may not do is escape the frame
// or be framed by anyone but the reader.
// privacyHeaders keep an unlisted link unlisted. The slug is the only thing
// standing between a document and the public, and a URL is easy to spill: a
// link in the document sends it to whatever site the reader clicks through to,
// and a crawler that finds it once has it for good.
const PRIVACY_HEADERS = {
  "referrer-policy": "no-referrer",
  "x-robots-tag": "noindex, nofollow, noarchive",
};

function documentHeaders(reader) {
  return {
    ...PRIVACY_HEADERS,
    "content-type": "text/html; charset=utf-8",
    "content-security-policy":
      "default-src 'self' data: blob: https:; " +
      "script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; " +
      "style-src 'self' 'unsafe-inline' data: https:; " +
      `frame-ancestors ${reader}; ` +
      "form-action 'none'; base-uri 'none'",
    "x-content-type-options": "nosniff",
    // Content-addressed path, so the bytes behind a URL never change.
    "cache-control": "public, max-age=31536000, immutable",
  };
}

// Documents live on a hostname of their own, a stranger to the reader, so a
// hostile document can reach neither its DOM nor its session. A different port
// would not do: cookies ignore ports.
//
// workers.dev hostnames are a single label, <script>.<subdomain>.workers.dev,
// so the second name cannot be a subdomain of the first. It is a second Worker
// instead, named <script>-docs, which `deploy` uploads alongside this one.
const DOCS_SUFFIX = "-docs";

function splitHost(url) {
  const [first, ...rest] = url.host.split(".");
  return { first, rest: rest.join(".") };
}

const isDocsHost = (url) => splitHost(url).first.endsWith(DOCS_SUFFIX);

function docsOrigin(url) {
  const { first, rest } = splitHost(url);
  const label = first.endsWith(DOCS_SUFFIX) ? first : first + DOCS_SUFFIX;
  return `${url.protocol}//${rest ? label + "." + rest : label}`;
}

function readerOrigin(url) {
  const { first, rest } = splitHost(url);
  const label = first.endsWith(DOCS_SUFFIX) ? first.slice(0, -DOCS_SUFFIX.length) : first;
  return `${url.protocol}//${rest ? label + "." + rest : label}`;
}

// withAgent appends the in-frame half of the reader. The stored bytes are
// never modified; the script is added on the way out.
function withAgent(html, reader) {
  const tag = `<script src="/agent.js?reader=${encodeURIComponent(reader)}"></script>`;
  const at = html.toLowerCase().lastIndexOf("</body>");
  return at >= 0 ? html.slice(0, at) + tag + html.slice(at) : html + tag;
}

// readUpload parses the two upload shapes -- a JSON body from the CLI and
// API clients, a multipart form from the browser -- into the fields a
// document needs. It returns those fields, or the Response to send back when
// the body cannot be read at all. Nothing here checks who is allowed to
// upload or where it will be stored; that is the storage half's job.
async function readUpload(request) {
  // JSON escaping can inflate a document, so the body is allowed to be
  // larger than MAX_HTML on the wire; the real check is on the decoded bytes
  // below. Refusing on the header keeps an oversized body from being read at
  // all, before a single byte of it is spent.
  const contentLength = Number(request.headers.get("content-length") || 0);
  if (contentLength > MAX_HTML * 2 + 1024) return json({ error: "document too large" }, 413);

  let title, slug, html, example = false, annotations = [];
  const type = request.headers.get("content-type") || "";
  if (type.includes("multipart/form-data")) {
    const form = await request.formData();
    title = form.get("title");
    slug = form.get("slug");
    const file = form.get("file");
    // Markdown is rendered in Go, which does not run here. The CLI renders it
    // before uploading, so it is only the browser upload that cannot.
    if (file && /\.(md|markdown)$/i.test(file.name || "")) {
      return json(
        {
          error:
            "this deployment cannot render markdown in the browser. " +
            "Publish it from the command line instead: komodoc publish " + (file.name || "file.md"),
        },
        415,
      );
    }
    html = file ? await file.text() : "";
  } else {
    const body = await request.json();
    ({ title, slug, html, example = false, annotations = [] } = body);
  }

  title = String(title || "").trim();
  html = String(html || "");
  if (!title || !html.trim()) return json({ error: "title and html are required" }, 400);

  // The byte length, not the character length: a document full of multi-byte
  // characters can be well under MAX_HTML in .length and over it in bytes,
  // which is what R2 and the quotas below actually charge for.
  const size = new TextEncoder().encode(html).byteLength;
  if (size > MAX_HTML) return json({ error: "document too large" }, 413);

  return { title, slug, html, example: Boolean(example), annotations, size };
}

// Thrown from inside updateIndex's mutate callback. The callback runs
// unguarded in updateIndex's retry loop, so throwing from it exits that loop
// immediately rather than being retried as though the ETag had merely lost a
// race -- a quota refusal is not a race to retry.
class QuotaRefusal {
  constructor(response) {
    this.response = response;
  }
}

// checkQuota enforces the deployment's storage ceilings against one snapshot
// of the index. Entries written before sizes were tracked count as zero
// bytes, so old documents can never by themselves blow a quota. It returns
// the refusal Response to send back, or null when the upload may proceed.
function checkQuota(entries, owner, key, size) {
  const existing = entries[key];
  const replacing = Boolean(existing);
  const sameOwner = replacing && (existing.publisher || "") === owner;
  const cutoff = Date.now() - 3600 * 1000;

  let total = 0, ownerBytes = 0, ownerDocs = 0, recentUploads = 0;
  for (const [entrySlug, entry] of Object.entries(entries)) {
    const entrySize = entry.size || 0;
    total += entrySize;
    if ((entry.publisher || "") === owner) {
      ownerBytes += entrySize;
      if (entrySlug !== key) ownerDocs++;
      if (Date.parse(entry.updated_at) >= cutoff) recentUploads++;
    }
  }
  // A replacement's old bytes are leaving, not staying, so they do not count
  // against the room the new version needs.
  if (replacing) total -= existing.size || 0;
  if (sameOwner) ownerBytes -= existing.size || 0;

  if (total + size > CONFIG.storage.total) {
    return json({ error: "this deployment has no room left" }, 507);
  }
  if (ownerBytes + size > CONFIG.storage.per_owner) {
    return json({ error: "your storage quota is used up; delete a document first" }, 507);
  }
  if (!replacing && ownerDocs >= CONFIG.storage.documents_per_owner) {
    return json({ error: "you have reached the document limit; delete one first" }, 507);
  }
  if (recentUploads >= CONFIG.storage.uploads_per_hour) {
    return json({ error: "too many uploads this hour; try later" }, 429);
  }
  return null;
}

// deleteStaleVersions removes every stored version of a document except the
// one the index now names. The reader only ever asks for the current digest,
// so nothing references an older one; on a document's first version there is
// nothing here to remove.
async function deleteStaleVersions(env, key, digest) {
  const keep = `documents/${key}/${digest}.html`;
  let cursor;
  do {
    const listing = await env.DOCS.list({ prefix: `documents/${key}/`, cursor });
    const stale = listing.objects.filter((object) => object.key !== keep);
    if (stale.length) await env.DOCS.delete(stale.map((object) => object.key));
    cursor = listing.truncated ? listing.cursor : undefined;
  } while (cursor);
}

async function handleUpload(request, env) {
  // Checked before the body is read, so an unauthorised upload costs nothing.
  const { owner, login, refusal } = await publisher(request, env);
  if (refusal) return refusal;

  const parsed = await readUpload(request);
  if (parsed instanceof Response) return parsed;
  const { title, slug, html, example, annotations, size } = parsed;

  const base = slugify(slug) || slugify(title);
  if (!base) return json({ error: "could not derive a slug" }, 400);

  // An exact slug that already exists is a replacement of that document, and
  // keeps its URL and its comments. Anything else is a new document, and gets a
  // random suffix so the link cannot be guessed from the title.
  const { entries: existingIndex } = await readIndex(env);

  // Reserved examples are installed only by the accounts a deployment names
  // with --examples. Their deterministic URL survives the hourly reset, and
  // an example publisher may replace any example; nobody else may touch one.
  if (example && !policyAllows(parsePolicy(env.KOMODOC_EXAMPLES), login)) {
    return json(
      { error: "only the deployment's example publishers may install reserved examples" }, 403);
  }
  if (example) {
    const annotationBytes = new TextEncoder().encode(JSON.stringify(annotations)).byteLength;
    if (annotationBytes > CONFIG.max_annotations) {
      return json({ error: "example annotations too large" }, 413);
    }
  }

  // Someone else's document is not yours to replace, and guessing its slug
  // should not even tell you it is there: a title that collides with another
  // publisher's document simply becomes a new document of your own.
  const replacing = existingIndex[base] && ownedBy(existingIndex[base], owner);
  const key = example || replacing ? base : `${base}-${randomSuffix()}`;

  // Checked against the index as just read, before an R2 write is spent on a
  // document that has nowhere to go. The CAS update below re-runs the same
  // check against whatever the index has become by the time it commits.
  const preflight = checkQuota(existingIndex, owner, key, size);
  if (preflight) return preflight;

  const digest = await sha256(html);
  const exampleRevision = example ? await sha256(JSON.stringify(annotations)) : "";
  await env.DOCS.put(`documents/${key}/${digest}.html`, html, {
    httpMetadata: { contentType: "text/html; charset=utf-8" },
  });

  const now = new Date().toISOString().replace(/\.\d+Z$/, "Z");
  let entries;
  try {
    entries = await updateIndex(env, (index) => {
      const refused = checkQuota(index, owner, key, size);
      if (refused) throw new QuotaRefusal(refused);
      const existing = index[key];
      index[key] = {
        slug: key,
        title,
        sha: digest,
        size,
        created_at: existing?.created_at || now,
        updated_at: now,
        // A replacement does not change hands.
        ...((existing?.publisher || owner) ? { publisher: existing?.publisher || owner } : {}),
        ...(example ? { example: true } : {}),
        ...(example ? { example_revision: exampleRevision } : {}),
      };
    });
  } catch (err) {
    if (!(err instanceof QuotaRefusal)) throw err;
    // The object was already written; a refusal here must not leave it
    // orphaned in R2 with nothing in the index pointing at it. Unless the
    // index already names it: a republish of unchanged content writes the
    // very object the live document is served from, and that one stays.
    if (existingIndex[key]?.sha !== digest) {
      await env.DOCS.delete(`documents/${key}/${digest}.html`);
    }
    return err.response;
  }

  await deleteStaleVersions(env, key, digest);
  if (example) {
    await env.DOCS.put(`examples/${key}.json`, JSON.stringify(annotations), {
      httpMetadata: { contentType: "application/json" },
    });
  }
  // Comments survive the replacement; they re-anchor in the reader.
  return json({ ...entries[key], url: `/docs/${key}` }, 201);
}

async function serveDocument(env, slug, sha, url) {
  if (!SLUG.test(slug) || !SHA.test(sha)) return new Response("not found", { status: 404 });
  const object = await env.DOCS.get(`documents/${slug}/${sha}.html`);
  if (!object) return new Response("not found", { status: 404 });
  const reader = readerOrigin(url);
  return new Response(withAgent(await object.text(), reader), { headers: documentHeaders(reader) });
}

// deleteDocumentObjects removes every stored object for a document -- every
// version under documents/<slug>/ -- without touching the index. Shared by a
// single delete and the janitor's sweep over many.
async function deleteDocumentObjects(env, slug) {
  let removed = 0;
  let cursor;
  do {
    const listing = await env.DOCS.list({ prefix: `documents/${slug}/`, cursor });
    if (listing.objects.length) await env.DOCS.delete(listing.objects.map((object) => object.key));
    removed += listing.objects.length;
    cursor = listing.truncated ? listing.cursor : undefined;
  } while (cursor);
  return removed;
}

async function deleteDocument(env, slug) {
  await room(env, slug).fetch(new Request("https://internal/purge"));
  const removed = await deleteDocumentObjects(env, slug);
  await updateIndex(env, (index) => { delete index[slug]; });
  return removed;
}

// The room for a document, given its already-looked-up index entry, or null
// when there is no such document. A room belongs to a document: without that
// check any invented slug would bring a fresh Durable Object into being, and
// since the comment rate limit is counted inside the room, a new slug per
// comment would mean no rate limit at all.
async function documentRoom(env, request, slug, entry) {
  if (!entry) return null;
  if (!entry.example) return room(env, slug);
  const login = request.headers.get("x-komodoc-login") || "";
  const visitor = await readVisitor(env, cookieValue(request, VISITOR_COOKIE));
  const identity = login ? `github:${login.toLowerCase()}` : `browser:${visitor || "missing"}`;
  const stub = room(env, `example:${slug}:${identity}`);
  await stub.fetch(new Request(
    `https://internal/ensure?slug=${encodeURIComponent(slug)}&revision=${entry.example_revision || ""}`,
  ));
  return stub;
}

// Purges every expired document's room and objects, then drops all of their
// index entries in a single compare-and-swap rather than one per document, so
// a sweep over many expired documents costs one write to index.json instead
// of many.
async function expireDocuments(env, scheduledTime) {
  const seconds = Number(env.KOMODOC_EXPIRE_SECONDS || 0);
  if (!(seconds > 0)) return;
  const from = env.KOMODOC_EXPIRE_FROM === "created" ? "created_at" : "updated_at";
  const cutoff = scheduledTime - seconds * 1000;
  const { entries } = await readIndex(env);
  const expired = Object.values(entries).filter(
    (entry) => !entry.example && Date.parse(entry[from]) <= cutoff);
  if (!expired.length) return;
  for (const entry of expired) {
    await room(env, entry.slug).fetch(new Request("https://internal/purge"));
    await deleteDocumentObjects(env, entry.slug);
  }
  await updateIndex(env, (index) => {
    for (const entry of expired) delete index[entry.slug];
  });
}

export default {
  async scheduled(controller, env, ctx) {
    ctx.waitUntil(expireDocuments(env, controller.scheduledTime));
  },
  async fetch(request, env) {
    const url = new URL(request.url);
    const path = url.pathname;
    const method = request.method;

    // --- the document origin -----------------------------------------------
    // Requests on docs.<host> get documents and the in-frame agent, and
    // nothing else: no shell, no API, no session.
    if (isDocsHost(url)) {
      let raw = path.match(/^\/raw\/([^/]+)\/([0-9a-f]{64})\.html$/);
      if (raw) return serveDocument(env, raw[1], raw[2], url);
      if (path === "/agent.js") return assetResponse(SHELL["/agent.js"]);
      return new Response("not found", { status: 404 });
    }

    // A document asked for on the reader's own host is sent to the other one.
    if (/^\/raw\/[^/]+\/[0-9a-f]{64}\.html$/.test(path)) {
      return Response.redirect(docsOrigin(url) + path, 302);
    }

    // --- signing in --------------------------------------------------------
    if (path.startsWith("/auth/") || path === "/api/me" || path === "/api/auth/config") {
      const answered = await handleAuth(request, env, url);
      if (answered) return answered;
    }

    // --- live comment channel -------------------------------------------
    let match = path.match(/^\/ws\/([^/]+)$/);
    if (match) {
      if (!SLUG.test(match[1])) return new Response("bad slug", { status: 400 });
      const { entries } = await readIndex(env);
      const identified = await withIdentity(request, env);
      const stub = await documentRoom(env, identified, match[1], entries[match[1]]);
      if (!stub) return new Response("not found", { status: 404 });
      return stub.fetch(identified);
    }

    // Stable, shareable URL: redirect to whichever version is current, on the
    // origin that serves documents.
    match = path.match(/^\/raw\/([^/]+)$/);
    if (match) {
      const { entries } = await readIndex(env);
      const entry = entries[match[1]];
      if (!entry) return new Response("not found", { status: 404 });
      return Response.redirect(`${docsOrigin(url)}/raw/${entry.slug}/${entry.sha}.html`, 302);
    }

    // --- api ---------------------------------------------------------------
    if (path === "/api/documents" && method === "POST") return handleUpload(request, env);

    // Listing is the one thing a link-holder must not be able to do: knowing
    // one document must not reveal the others, so it takes a publisher.
    if (path === "/api/list" && (method === "POST" || method === "GET")) {
      const { owner, refusal } = await publisher(request, env);
      if (refusal) return refusal;
      const { entries } = await readIndex(env);
      const docs = visibleTo(Object.values(entries), owner)
        .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      return json({ documents: docs });
    }

    // Deleting one document: its stored versions, its index entry, and the
    // comments in its Room. Gated like publishing.
    match = path.match(/^\/api\/documents\/([^/]+)\/delete$/);
    if (match && method === "POST") {
      const slug = match[1];
      if (!SLUG.test(slug)) return json({ error: "bad slug" }, 400);
      const { owner, refusal } = await publisher(request, env);
      if (refusal) return refusal;
      const { entries } = await readIndex(env);
      // Another publisher's document answers exactly as a missing one does, so
      // a guessed slug reveals nothing.
      if (!entries[slug]) return json({ error: "not found" }, 404);
      if (entries[slug].example) return json({ error: "reserved examples cannot be deleted" }, 403);
      if (!ownedBy(entries[slug], owner)) return json({ error: "not found" }, 404);
      const title = entries[slug].title;

      const removed = await deleteDocument(env, slug);
      return json({ deleted: slug, title, versions_removed: removed });
    }

    match = path.match(/^\/api\/documents\/([^/]+)$/);
    if (match && method === "GET") {
      const { entries } = await readIndex(env);
      const entry = entries[match[1]];
      if (!entry) return json({ error: "not found" }, 404);
      const identified = await withIdentity(request, env);
      const counts = await (await documentRoom(env, identified, match[1], entry))
        .fetch(new Request(`${url.origin}/counts`))
        .then((response) => response.json());
      return json({ ...entry, ...counts, docs_origin: docsOrigin(url) });
    }

    // REST fallbacks, used when the socket is unavailable.
    match = path.match(/^\/api\/documents\/([^/]+)\/comments$/);
    if (match) {
      if (!SLUG.test(match[1])) return json({ error: "bad slug" }, 400);
      const { entries } = await readIndex(env);
      const identified = await withIdentity(request, env);
      const stub = await documentRoom(env, identified, match[1], entries[match[1]]);
      if (!stub) return json({ error: "not found" }, 404);
      return stub.fetch(identified);
    }

    // --- the shell ---------------------------------------------------------
    // Served from constants compiled into this script rather than from a
    // static-asset binding, so deploying is one script upload and nothing else.
    const page = SHELL[path]
      ? path
      : /^\/docs\/[^/]+$/.test(path)
        ? "/reader.html"
        : path === "/"
          ? "/index.html"
          : path;
    const asset = SHELL[page];
    if (asset) {
      const response = assetResponse(asset);
      // Every page names the browser, not just a reader: the index page is
      // where an upload starts, and it needs an owner to belong to. A cookie
      // that does not verify -- absent, or the old unsigned form a browser
      // this Worker signed cookies for might still hold -- is treated as
      // though there were none, and simply replaced.
      if (asset.type?.startsWith("text/html") &&
          !(await readVisitor(env, cookieValue(request, VISITOR_COOKIE)))) {
        const signed = await signVisitor(env, crypto.randomUUID());
        response.headers.append("set-cookie", `${VISITOR_COOKIE}=${signed}; Path=/; Max-Age=31536000; HttpOnly; Secure; SameSite=Lax`);
        // A shared cache handing this same identity to the next browser would
        // defeat the point of having one.
        response.headers.set("cache-control", "private, no-store");
      }
      return response;
    }
    return new Response("not found", { status: 404 });
  },
};

// withIdentity copies a request, adding the caller's verified login as a
// header. The Room is inside the trust boundary, so this is the only place the
// name on a comment can come from when commenting needs an account.
async function withIdentity(request, env) {
  const login = await whoami(request, env);
  const headers = new Headers(request.headers);
  headers.set("x-komodoc-login", login || "");
  return new Request(request, { headers });
}

// handleAuth serves the sign-in routes, or returns null when the path is not
// one of them.
async function handleAuth(request, env, url) {
  const clientID = env.KOMODOC_GITHUB_CLIENT_ID || "";
  const redirect = `${url.origin}/auth/callback`;

  if (url.pathname === "/auth/login") {
    const state = crypto.randomUUID();
    const next = url.searchParams.get("next") || "/";
    const authorize = new URL("https://github.com/login/oauth/authorize");
    authorize.searchParams.set("client_id", clientID);
    authorize.searchParams.set("redirect_uri", redirect);
    authorize.searchParams.set("state", state);
    return new Response(null, {
      status: 302,
      headers: {
        location: authorize.toString(),
        "set-cookie": `komodoc_state=${state}|${next}; Path=/; Max-Age=600; HttpOnly; Secure; SameSite=Lax`,
      },
    });
  }

  if (url.pathname === "/auth/callback") {
    const [state, next = "/"] = (cookieValue(request, "komodoc_state") || "").split("|");
    // The state ties this callback to the redirect that started it.
    if (!state || url.searchParams.get("state") !== state) {
      return new Response("sign-in state did not match; try again", { status: 400 });
    }
    const exchange = await fetch("https://github.com/login/oauth/access_token", {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({
        client_id: clientID,
        client_secret: env.KOMODOC_GITHUB_CLIENT_SECRET || "",
        code: url.searchParams.get("code"),
        redirect_uri: redirect,
      }),
    });
    const token = (await exchange.json().catch(() => ({}))).access_token;
    if (!token) return new Response("github refused the sign-in", { status: 400 });
    const login = await githubLogin(token);
    if (!login) return new Response("github would not say who you are", { status: 502 });

    const expiry = Math.floor(Date.now() / 1000) + 30 * 24 * 3600;
    const session = await makeSession(env, login, expiry);
    return new Response(null, {
      status: 302,
      headers: [
        ["location", localPath(next)],
        ["set-cookie", `komodoc_session=${session}; Path=/; Max-Age=${30 * 24 * 3600}; HttpOnly; Secure; SameSite=Lax`],
        ["set-cookie", "komodoc_state=; Path=/; Max-Age=0"],
      ],
    });
  }

  if (url.pathname === "/auth/logout") {
    return new Response(null, {
      status: 302,
      headers: { location: "/", "set-cookie": "komodoc_session=; Path=/; Max-Age=0" },
    });
  }

  if (url.pathname === "/api/me") {
    const login = await whoami(request, env);
    const publishers = parsePolicy(env.KOMODOC_PUBLISHERS);
    const commenters = parsePolicy(env.KOMODOC_COMMENTERS);
    return json({
      login,
      can_publish: policyAllows(publishers, login),
      can_comment: policyAllows(commenters, login),
      comments_need_login: !commenters.public,
      can_sign_in: Boolean(clientID),
      examples_enabled: examplesEnabled(env),
      publishers: describePolicy(publishers),
      commenters: describePolicy(commenters),
    });
  }

  // The client id is public by design; the CLI asks for it so `login` needs no
  // configuration of its own.
  if (url.pathname === "/api/auth/config") return json({ client_id: clientID });

  return null;
}

// One shell file. A binary travels as base64, because the shell is embedded as
// JSON and JSON holds no bytes; a file whose contents never change is cached
// for a year rather than five minutes.
function assetResponse(asset) {
  let body = asset.body;
  if (asset.base64) {
    const binary = atob(asset.body);
    body = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) body[i] = binary.charCodeAt(i);
  }
  return new Response(body, {
    headers: {
      "content-type": asset.type,
      ...PRIVACY_HEADERS,
      "cache-control": asset.immutable
        ? "public, max-age=31536000, immutable"
        : "public, max-age=300",
    },
  });
}
