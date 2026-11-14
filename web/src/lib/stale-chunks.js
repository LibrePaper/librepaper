// Reload once when a lazily loaded chunk has gone missing.
//
// A deploy replaces the bundles the server embeds, so a tab opened before it
// asks for a hashed file the new release no longer has, and the server answers
// 404. Vite reports that failed dynamic import as `vite:preloadError` on the
// window. Reloading picks up the new release. The document is safe to reload:
// edits are kept in the browser's own store and resent.
//
// The stamp in storage stops a genuinely missing file from looping reloads:
// a second failure within a minute is left to surface as an error.

const KEY = "librepaper-stale-chunk-reload";
const WINDOW_MS = 60_000;

function defaultStorage() {
  try {
    return globalThis.sessionStorage;
  } catch {
    return null;
  }
}

// The last reload's time, null when nothing is stored, and undefined when
// storage cannot be read at all.
function readStamp(storage) {
  if (!storage) return undefined;
  try {
    const value = storage.getItem(KEY);
    return value == null ? null : Number(value);
  } catch {
    return undefined;
  }
}

export function reloadOnStaleChunks({
  win = globalThis.window,
  storage = defaultStorage(),
  now = () => Date.now(),
} = {}) {
  // Only used when storage cannot hold the stamp, so a page with no storage
  // still reloads once and no more.
  let reloadedHere = false;
  win.addEventListener("vite:preloadError", (event) => {
    const time = now();
    const stamp = readStamp(storage);
    if (stamp === undefined || stamp === null) {
      if (reloadedHere) return;
    } else if (time - stamp < WINDOW_MS) {
      return;
    }
    reloadedHere = true;
    if (stamp !== undefined) {
      try {
        storage.setItem(KEY, String(time));
      } catch {
        // The flag above still holds for this page load.
      }
    }
    event.preventDefault();
    win.location.reload();
  });
}
