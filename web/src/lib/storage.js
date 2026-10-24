// What this browser remembers on its own: which documents are starred, when
// each was last opened, how wide the panes are. None of it belongs to a
// document -- it is one reader's own -- so none of it leaves the browser.
//
// Storage can be switched off, and a page that throws when it is would be a
// page that does not load at all. Every read has a fallback and every write
// is allowed to fail.

import { localDataForgotten } from "./signed-out.js";

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

// Every agent chat transcript is kept under this prefix, one key per document.
export const AGENT_SESSION_PREFIX = "librepaper.agent.session.";
export const VIEWED = "librepaper-viewed";
export const LINKED = "librepaper-linked";
// Whether PDF pages are drawn inverted while the application is dark. A habit
// of the person reading rather than of the document.
export const PDF_INVERT = "librepaper-pdf-invert";
// Which side the source was on. A habit of the reader rather than anything
// about a document, so reopening an editor puts the source where they left it.
export const SOURCE_SIDE = "librepaper-source-side";
// The link keys this browser has been given, by slug. A key is a secret, and
// this is the right place for one: it is per browser, so opening the link on a
// phone means pasting it again, and forgetLocalData removes it on sign-out.
export const KEYS = "librepaper-keys";
// Which keys the editor answers to: "default", "vim" or "emacs". A habit of the
// person typing rather than of the document, so it lives here rather than on
// the shared text, and a coauthor on the other end keeps whichever they chose.
export const KEYMAP = "librepaper-keymap";
// The source text size in pixels. Like the keymap, a habit of the person
// looking at the source rather than of the document.
export const EDITOR_SIZE = "librepaper-editor-size";
// The source font as { id, custom }. Like the size, a habit of the person looking at the source.
export const EDITOR_FONT = "librepaper-editor-font";
// The source line height: "compact", "normal" or "wide". Like the size, a habit of the reader.
export const EDITOR_LINE_HEIGHT = "librepaper-editor-line-height";
// Whether suggestions pop up while typing in the source. Like the keymap, a
// habit of the person typing.
export const EDITOR_COMPLETION = "librepaper-editor-completion";
// Whether typing an opening bracket adds the closing one.
export const EDITOR_BRACKETS = "librepaper-editor-brackets";
// Whether the browser underlines misspelled words in the source. Prose files only.
export const EDITOR_SPELLCHECK = "librepaper-editor-spellcheck";
// Whether compiler messages are drawn in the source: underlines and gutter
// marks. The list of messages stays available either way.
export const EDITOR_CHECK = "librepaper-editor-check";
// Whether the pages are light, dark, or follow the computer: "system", "light"
// or "dark". Like the keymap, a habit of the person looking at the screen.
export const THEME = "librepaper-theme";
// Which colour theme the pages use: "librepaper", "flexoki", "solarized",
// "gruvbox", "catppuccin" or "nord". Like the theme, a habit of the person
// looking at the screen rather than of the document.
export const PALETTE = "librepaper-palette";
// The symbol ids the picker used last, most recent first.
export const SYMBOLS_RECENT = "librepaper-symbols-recent";
// The symbol ids starred in the picker, in the order they were starred.
export const SYMBOLS_FAVORITES = "librepaper-symbols-favorites";

// A page can keep using a link key even when persistent storage is denied. The
// value lasts only for this page, which is the same lifetime as the fragment
// key before it is removed from the address bar.
const memoryKeys = new Map();

/// Takes the link key out of the address, keeps it, and cleans the address
/// bar. A link carries its key in one of two places. The current form puts it
/// in the path, as /docs/<slug>/<key>, so it survives apps that drop the
/// fragment when they open a link. The older form puts it in the fragment, as
/// /docs/<slug>#k=<key>. Once it is here, the visible URL has no reason to keep
/// carrying it, and "Copy link" puts it back.
export function takeLinkKey(slug) {
  const parts = location.pathname.split("/");
  const inPath = parts.length === 4 && parts[1] === "docs" && parts[2] === slug && parts[3] !== "";
  const raw = inPath ? parts[3] : /(?:^|[#&])k=([^&]+)/.exec(location.hash || "")?.[1];
  if (raw === undefined) return keyFor(slug);
  // The path form keeps any fragment the address had, as it always has.
  const hash = inPath ? location.hash : "";
  const address = `${inPath ? `/docs/${slug}` : location.pathname}${location.search}${hash}`;
  let key;
  try {
    key = decodeURIComponent(raw);
  } catch {
    // A malformed escape is untrusted URL input. Remove it from the address
    // bar just like a valid key, but never let it prevent the reader loading.
    history.replaceState(null, "", address);
    return keyFor(slug);
  }
  memoryKeys.set(slug, key);
  // After a sign-out the key serves this page only; it is not stored again.
  if (!localDataForgotten()) {
    const keys = read(KEYS, {});
    keys[slug] = key;
    write(KEYS, keys);
  }
  history.replaceState(null, "", address);
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
/// copied from the reader is the link that was given. The key goes in the path,
/// as /docs/<slug>/<key>, so chat apps that drop the fragment still keep it.
export function linkFor(slug) {
  const key = keyFor(slug);
  const here = new URL(`/docs/${slug}`, location.origin).href;
  return key ? `${here}/${encodeURIComponent(key)}` : here;
}

/// Notes that this document was opened just now, which is what the landing
/// page sorts by.
export function markViewed(slug) {
  if (localDataForgotten()) return;
  const seen = read(VIEWED, {});
  seen[slug] = new Date().toISOString();
  write(VIEWED, seen);
}
