// A small in-page record of what went wrong, for the person who wants to report it.
//
// It lives in memory only. It survives the error that was recorded, not a
// reload, and nothing here is sent anywhere. Settings can show it and the user
// can copy it out. Every entry is built from fields chosen by the caller, so
// callers decide what is safe to keep.

const LIMIT = 200;

/// Oldest first. Past LIMIT the oldest entry is dropped.
const buffer = [];

/// Notes one entry. `entry.kind` is "error", "warning" or "request".
export function record(entry) {
  buffer.push({ at: Date.now(), ...entry });
  if (buffer.length > LIMIT) buffer.splice(0, buffer.length - LIMIT);
}

/// A copy of the buffer, oldest first.
export function entries() {
  return buffer.map((entry) => ({ ...entry }));
}

/// Forgets everything recorded so far.
export function clear() {
  buffer.length = 0;
}
