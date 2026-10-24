import { forgetLocalData } from "./local-data.js";

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

export const getPrivate = (path) => fetch(path, { headers: SHELL_HEADERS }).then(json);

export const post = (path, body) =>
  fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", ...SHELL_HEADERS },
    body: JSON.stringify(body ?? {}),
  }).then(json);

// With onprogress the request goes through XMLHttpRequest, the only browser
// API that reports how much of an upload has been sent; the answer has the
// same shape the callers read from fetch.
/** @param {FormData} form @param {{ onprogress?: (loaded: number, total: number) => void }} [options] */
export function upload(form, { onprogress } = {}) {
  if (!onprogress) return fetch("/api/documents", { method: "POST", headers: SHELL_HEADERS, body: form });
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open("POST", "/api/documents");
    for (const [name, value] of Object.entries(SHELL_HEADERS)) request.setRequestHeader(name, value);
    request.upload.onprogress = (event) => onprogress(event.loaded, event.total);
    request.onload = () => resolve({
      ok: request.status >= 200 && request.status < 300,
      status: request.status,
      json: () => new Promise((parsed, failed) => {
        try { parsed(JSON.parse(request.responseText)); }
        catch (error) { failed(error); }
      }),
    });
    request.onerror = () => reject(new Error("The upload failed. Check your connection and try again."));
    request.onabort = () => reject(new Error("The upload was interrupted."));
    request.send(form);
  });
}

/// The limits both this page and the server enforce, so the two never
/// disagree about what will be refused.
export const config = () => get("/api/config");

/// Who you are, which decides what every page is: what you may publish, what
/// you may comment on, and which providers there are to sign in with. The
/// answer carries `provider`, `handle` and `name`: the handle is what the
/// deployment's switches match and is your own to see, the name is what other
/// readers see, and for a Google account those are deliberately not the same
/// string. `providers` lists the sign-in choices as `{id, kind, label}`
/// objects, in sign-in page order. It is empty on a deployment with no
/// sign-in at all.
export const me = () => get("/api/me").catch(() => ({}));

/// Where a signed-out reader lands: the sign-in page, marked as signed out so
/// a single provider is not followed straight back in. Any page that would
/// send a signed-out reader to "/" must use this too, or "/" sends them to
/// sign-in unmarked and they are signed straight back in.
export const signedOutHref = () => "/auth/login?signed_out=1";

/// Signs out and leaves.
export async function signOut() {
  // A GET can be forced onto a signed-in reader cross-site, so signing out is
  // a POST carrying the same header every other state change does.
  const response = await fetch("/auth/logout", { method: "POST", headers: SHELL_HEADERS }).catch(() => null);
  // Only a confirmed sign-out clears what this browser kept. If the request
  // failed, the session cookie survives and so must the local copies.
  if (response?.ok) await forgetLocalData();
  // The sign-in page, not this one. Reloading left you wherever you happened
  // to be signing out from -- a project you can no longer open, a panel with
  // nothing in it -- and somebody who has just signed out is a stranger
  // again, so they get what a stranger gets. `replace`, so Back does not
  // return to a page that is now somebody else's.
  location.replace(signedOutHref());
}

/// Asks the server to erase the signed-in account: what it owns is queued for
/// deletion, its authorship elsewhere is unlinked, and the session is
/// invalidated on the spot -- so this is the last request this browser makes
/// as that account. A POST carrying the same header every state change does.
export const eraseAccount = () => post("/api/account/erase");

/// The one door. Which providers this deployment has is the server's business:
/// this address is a redirect when there is one and a choice when there are
/// two, so no page has to render a button per provider.
export const signInHref = (next = location.pathname + location.search) =>
  `/auth/login?next=${encodeURIComponent(next)}`;

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

/// A document URL without its link key. The key lives in the fragment of an
/// older link, and in the path segment after the slug of a current one, so both
/// are cut here. The local service gets this form: it is given its own token,
/// never the key.
export function withoutKey(url) {
  const parsed = new URL(url, globalThis.location?.href);
  parsed.hash = "";
  const parts = parsed.pathname.split("/");
  if (parts.length === 4 && parts[1] === "docs") parsed.pathname = `/docs/${parts[2]}`;
  return parsed.href;
}
