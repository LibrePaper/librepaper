// Per-browser local code execution permissions. When a user enables code
// execution for a document, that grant is remembered in localStorage until
// the user revokes it or signs out. Grants are scoped to (origin, user, slug)
// so a different person or a different document never inherits a permission.
// Signing out forgets every grant, because the person returning to this
// browser later will be a stranger, and documents must ask permission afresh.

const PREFIX = "librepaper-local-execution-v1";

function storage() {
  return typeof localStorage === "undefined" ? null : localStorage;
}

function makeKey(origin = globalThis.location?.origin || "", user = "anonymous", slug = "") {
  return `${PREFIX}:${JSON.stringify([String(origin), String(user || "anonymous"), String(slug)])}`;
}

export function granted({ origin = globalThis.location?.origin || "", user = "anonymous", slug = "" } = {}) {
  if (!slug) return false;
  try {
    const key = makeKey(origin, user, slug);
    return storage()?.getItem(key) === "1";
  } catch {
    return false;
  }
}

export function grant({ origin = globalThis.location?.origin || "", user = "anonymous", slug = "" } = {}) {
  if (!slug) return;
  try {
    const key = makeKey(origin, user, slug);
    storage()?.setItem(key, "1");
  } catch {
    // private mode or quota exceeded
  }
}

export function revoke({ origin = globalThis.location?.origin || "", user = "anonymous", slug = "" } = {}) {
  try {
    const key = makeKey(origin, user, slug);
    storage()?.removeItem(key);
  } catch {
    // private mode
  }
}

export function revokeAll() {
  try {
    const store = storage();
    if (!store) return;
    const keys = [];
    for (let i = 0; i < store.length; i++) {
      const key = store.key(i);
      if (key?.startsWith(PREFIX)) keys.push(key);
    }
    keys.forEach((key) => store.removeItem(key));
  } catch {
    // private mode
  }
}

export function isGrantKey(key) {
  return typeof key === "string" && key.startsWith(PREFIX);
}
