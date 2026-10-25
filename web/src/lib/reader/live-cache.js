// The decisions behind keeping a live preview's page. A page that Quarto or
// Calepin produced is stored with the source it was rendered from, and it is
// shown again, without starting the engine, when the source is unchanged.
// Kept apart from the Reader so the rules can be read and tested alone.

/// The name a live page is kept under: which engine made it, in which output,
/// from which source. The browser's own draft of the same source is stored
/// under the same key with a different name, so a draft is never taken for a
/// page the engine made. An empty digest means no sync has completed for the
/// current target yet, and the page has no name.
export function stamp(engine, output, digest) {
  if (!engine || !digest) return "";
  return `live:${engine}:${output || ""}:${digest}`;
}

/// Whether a kept record is the page for this identity.
export function matches(record, identity) {
  if (!identity || !record || record.identity !== identity) return false;
  if (record.kind === "pdf") return Boolean(record.bytes?.byteLength);
  return record.kind === "html" && typeof record.html === "string";
}

/// The first of `records` that matches, or null.
export function pick(records, identity) {
  for (const record of records) if (matches(record, identity)) return record;
  return null;
}

/// Whether the live preview may start for `path`. It may not while a kept page
/// is held for the file, or while the lookup for one is still under way.
export function shouldStartLive({ held = "", lookup = "", path = "" } = {}) {
  if (!path) return false;
  return held !== path && lookup !== path;
}

/// Whether the pane is being held for `path`, by a page or by a lookup.
export function holding({ held = "", lookup = "", path = "" } = {}) {
  return Boolean(path) && (held === path || lookup === path);
}
