// The editor place of one project in one tab: which face and file were open,
// where the caret was, and how far the document was scrolled.
//
// A phone browser discards a backgrounded tab and reloads it when the person
// comes back, so the place is written while the page is hidden and read once
// after the reload. It is kept in sessionStorage, which belongs to this tab
// alone: a new tab or a shared link starts fresh, and nothing goes to the
// server, because the place belongs to this device.

function defaultStorage() {
  try {
    return globalThis.sessionStorage;
  } catch {
    return null;
  }
}

const key = (slug) => `librepaper-place:${slug}`;

// Offsets are whole, non-negative character positions. Anything else is zero:
// a place that cannot be read should not move the caret somewhere odd.
function offset(value) {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? Math.floor(number) : 0;
}

function text(value) {
  return typeof value === "string" ? value : "";
}

export function savePlace(slug, place, storage = defaultStorage()) {
  if (!storage) return;
  try {
    storage.setItem(key(slug), JSON.stringify(place));
  } catch {
    // Storage that refuses the write leaves the place unsaved: the reload
    // then starts where a fresh visit would.
  }
}

export function readPlace(slug, storage = defaultStorage()) {
  if (!storage) return null;
  let saved;
  try {
    saved = storage.getItem(key(slug));
  } catch {
    return null;
  }
  if (saved == null) return null;
  let value;
  try {
    value = JSON.parse(saved);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  // Without a file there is nothing to put back.
  if (typeof value.openFile !== "string" || value.openFile === "") return null;
  return {
    openFile: value.openFile,
    caret: offset(value.caret),
    top: offset(value.top),
    offset: offset(value.offset),
    layout: text(value.layout),
    preferredPane: text(value.preferredPane),
    mobileView: text(value.mobileView),
    link: text(value.link),
  };
}
