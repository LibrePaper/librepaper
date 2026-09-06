// What this browser remembers on its own: which documents are starred, when
// each was last opened, how wide the panes are. None of it belongs to a
// document -- it is one reader's own -- so none of it leaves the browser.
//
// Storage can be switched off, and a page that throws when it is would be a
// page that does not load at all. Every read has a fallback and every write
// is allowed to fail.

export function read(key, fallback) {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : JSON.parse(raw);
  } catch {
    return fallback;
  }
}

export function write(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* it still applies to this page */
  }
}

export const VIEWED = "komodoc-viewed";
export const FAVORITES = "komodoc-favorites";
export const LINKED = "komodoc-linked";
// How the window was last divided, and which side the source was on. Both are
// one reader's habit rather than anything about a document, so reopening an
// editor lands where they left it.
export const LAYOUT = "komodoc-layout";
export const SOURCE_SIDE = "komodoc-source-side";
// Which panel the column last showed -- the files, the comments or the history
// -- or "" for closed. Absent on a first visit, which opens on the files.
export const PANEL = "komodoc-panel";
// The link keys this browser has been given, by slug. A key is a secret, and
// this is the right place for one: it is per browser, so opening the link on a
// phone means pasting it again, and it is cleared with everything else.
export const KEYS = "komodoc-keys";

// A page can keep using a link key even when persistent storage is denied. The
// value lasts only for this page, which is the same lifetime as the fragment
// key before it is removed from the address bar.
const memoryKeys = new Map();

/// Takes the key out of the URL fragment, keeps it, and cleans the address
/// bar. A fragment is never sent to a server, so the key lands in nobody's
/// access log and on no Referer header; once it is here, the visible URL has
/// no reason to keep carrying it, and "Copy link" puts it back.
export function takeKeyFromFragment(slug) {
  const found = /(?:^|[#&])k=([^&]+)/.exec(location.hash || "");
  if (!found) return keyFor(slug);
  const key = decodeURIComponent(found[1]);
  memoryKeys.set(slug, key);
  const keys = read(KEYS, {});
  keys[slug] = key;
  write(KEYS, keys);
  history.replaceState(null, "", location.pathname + location.search);
  return key;
}

/// The key this browser holds for a document, or "" for none.
export function keyFor(slug) {
  if (memoryKeys.has(slug)) return memoryKeys.get(slug);
  const keys = read(KEYS, {});
  const key = typeof keys[slug] === "string" ? keys[slug] : "";
  if (key) memoryKeys.set(slug, key);
  return key;
}

/// The link to hand somebody: the one that was shared, key and all, so a link
/// copied from the reader is the link that was given.
export function linkFor(slug) {
  const key = keyFor(slug);
  const here = new URL(`/docs/${slug}`, location.origin).href;
  return key ? `${here}#k=${encodeURIComponent(key)}` : here;
}

/// Notes that this document was opened just now, which is what the landing
/// page sorts by.
export function markViewed(slug) {
  const seen = read(VIEWED, {});
  seen[slug] = new Date().toISOString();
  write(VIEWED, seen);
}
