// A small in-page record of what went wrong, for the person who wants to report it.
//
// It is kept in this browser tab's sessionStorage, so it survives a reload of
// the same tab (the crash page's Reload button keeps the evidence) and is gone
// when the window closes. It never leaves the device, and nothing here is sent
// anywhere. Settings can show it and the user can copy it out. Every entry is
// built from fields chosen by the caller, so callers decide what is safe to keep.
//
// Where sessionStorage is missing or throws (workers, private modes, tests),
// the buffer lives in memory for the life of the page and nothing else changes.

const LIMIT = 200;
const KEY = "librepaper.support-log";

/// The session store, or null where the page has none. Reading the global
/// itself can throw in sandboxed frames, so that is guarded too.
function session() {
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}

/// The entries saved by an earlier load of this tab, oldest first. Anything
/// unreadable is ignored.
function restore() {
  try {
    const raw = session()?.getItem(KEY);
    if (!raw) return [];
    const saved = JSON.parse(raw);
    if (!Array.isArray(saved)) return [];
    return saved.filter((entry) => entry && typeof entry === "object" && !Array.isArray(entry)).slice(-LIMIT);
  } catch {
    return [];
  }
}

/// Oldest first. Past LIMIT the oldest entry is dropped.
const buffer = restore();

// Writes are batched: records made in one task are saved together, in a
// microtask, which runs before the page can be reloaded.
let saving = false;

function save() {
  if (saving) return;
  saving = true;
  queueMicrotask(() => {
    saving = false;
    try {
      const store = session();
      if (!store) return;
      if (buffer.length) store.setItem(KEY, JSON.stringify(buffer));
      else store.removeItem(KEY);
    } catch {
      // Full, blocked or not serializable: the buffer stays in memory only.
    }
  });
}

/// Notes one entry. `entry.kind` is "error", "warning" or "request".
export function record(entry) {
  buffer.push({ at: Date.now(), ...entry });
  if (buffer.length > LIMIT) buffer.splice(0, buffer.length - LIMIT);
  save();
}

/// A copy of the buffer, oldest first.
export function entries() {
  return buffer.map((entry) => ({ ...entry }));
}

/// Forgets everything recorded so far, here and in the session store.
export function clear() {
  buffer.length = 0;
  try {
    session()?.removeItem(KEY);
  } catch {
    // Nothing to remove if the store cannot be reached.
  }
}
