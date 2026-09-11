// Talking to the server.
//
// A browser cannot set a custom header on a cross-origin request without a
// CORS preflight, which the server never grants -- so this header is proof, to
// the server, that a state-changing request came from this page and not from a
// hostile document on the sibling documents host. It goes on every write and
// on the listing, never on plain navigation.
export const SHELL_HEADERS = { "X-LibrePaper-Client": "shell" };

async function json(response) {
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `${response.status}`);
  }
  return response.json();
}

// The link key a reader arrived with, presented on every request for that
// document. A header rather than a query parameter, so it never reaches an
// access log; a browser cannot set one cross-origin without a preflight the
// server never grants, so this is proof it came from this page.
export const KEY_HEADER = "X-LibrePaper-Key";

export const keyHeaders = (key) => (key ? { [KEY_HEADER]: key } : {});

/// The headers a keyed call to the server carries: the shell marker always,
/// the link key when there is one, and -- when the call has a JSON body --
/// a content-type. Header names are case-insensitive on the wire, so this
/// is safe to use wherever a site built the same three headers by hand,
/// whatever case or order it happened to spell them in.
export const authHeaders = (key, contentType) => ({
  ...(contentType ? { "content-type": contentType } : {}),
  ...SHELL_HEADERS,
  ...keyHeaders(key),
});

export const get = (path) => fetch(path).then(json);

/// A read on behalf of somebody holding a link.
export const getKeyed = (path, key) => fetch(path, { headers: keyHeaders(key) }).then(json);

/// A write on behalf of somebody holding a link.
export const postKeyed = (path, body, key) =>
  fetch(path, {
    method: "POST",
    headers: authHeaders(key, "application/json"),
    body: JSON.stringify(body ?? {}),
  }).then(json);

export const getPrivate = (path) => fetch(path, { headers: SHELL_HEADERS }).then(json);

export const post = (path, body) =>
  fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", ...SHELL_HEADERS },
    body: JSON.stringify(body ?? {}),
  }).then(json);

/// A POST whose status matters to the caller, since a refused save says what
/// to do about it and a thrown error would lose that.
export const postRaw = (path, body) =>
  fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", ...SHELL_HEADERS },
    body: JSON.stringify(body),
  });

export const upload = (form) =>
  fetch("/api/documents", { method: "POST", headers: SHELL_HEADERS, body: form });

/// The limits both this page and the server enforce, so the two never
/// disagree about what will be refused.
export const config = () => get("/api/config");

/// Who you are, which decides what every page is: what you may publish, what
/// you may comment on, and which providers there are to sign in with. The
/// answer carries `provider`, `handle` and `name`: the handle is what the
/// deployment's switches match and is your own to see, the name is what other
/// readers see, and for a Google account those are deliberately not the same
/// string. `providers` is empty on a deployment with no sign-in at all.
export const me = () => get("/api/me").catch(() => ({}));

export async function signOut() {
  // A GET can be forced onto a signed-in reader cross-site, so signing out is
  // a POST carrying the same header every other state change does.
  await fetch("/auth/logout", { method: "POST", headers: SHELL_HEADERS }).catch(() => {});
  location.reload();
}

/// The one door. Which providers this deployment has is the server's business:
/// this address is a redirect when there is one and a choice when there are
/// two, so no page has to render a button per provider.
export const signInHref = () => `/auth/login?next=${encodeURIComponent(location.pathname)}`;

/// Puts a figure on the server and answers with its digest and size. The bytes
/// go up as they are -- a figure is not JSON and wrapping it in base64 would
/// cost a third of its size on the wire -- and the name is not sent at all:
/// the server keeps the bytes under their digest, and the shared document is
/// where the name is written, by whoever uploaded it, a moment later.
export const uploadAsset = (slug, file, key) =>
  fetch(`/api/documents/${slug}/assets`, {
    method: "PUT",
    headers: authHeaders(key),
    body: file,
  }).then(json);
