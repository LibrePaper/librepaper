// Per-browser build preferences: one per format for each person in this
// browser, shared by every document of that format. These values never enter
// collaborative state.
const PREFIX = "librepaper-build-v3";
const ANONYMOUS_KEY = `${PREFIX}:anonymous-id`;
let anonymousId;

const DEFAULTS = Object.freeze({ selection: "automatic", backend: "auto", output: "" });

function storage() { return typeof localStorage === "undefined" ? null : localStorage; }
export function anonymousIdentity() {
  if (anonymousId) return anonymousId;
  try {
    const current = storage()?.getItem(ANONYMOUS_KEY);
    if (current) return (anonymousId = current);
    const value = globalThis.crypto?.randomUUID?.() || `anonymous-${Math.random().toString(36).slice(2)}`;
    storage()?.setItem(ANONYMOUS_KEY, value);
    return (anonymousId = value);
  } catch { return (anonymousId = "anonymous"); }
}
function key({ origin = globalThis.location?.origin || "", user = "anonymous" } = {}, what = "") {
  return `${PREFIX}:${JSON.stringify([String(origin), String(user || "anonymous"), String(what)])}`;
}

/// HTML for every format. A preview is read on a screen, and a flow page
/// reflows to the pane it is read in, arrives faster than a paged compile, and
/// is what the annotation layer can place a highlight in. A paged PDF is what
/// LaTeX and Typst are ultimately for, so it stays one choice away in the
/// View menu -- but it is a choice, not the starting point.
export function defaults(format = "") { return { ...DEFAULTS, format, output: "html" }; }

/// And the starting point is where every visit starts. Unlike the tool and the
/// backend, which say how this browser can build at all and are worth keeping,
/// the output is a choice about what to look at right now: somebody who asked
/// for pages once to check a figure's placement does not want every later
/// visit to open on a paged compile. So the output is not written, and not
/// read back -- opening a project is always opening it on HTML, and PDF lasts
/// as long as the visit that asked for it.

// The Quarto profile and parameters. Markdown rendered by Quarto takes the
// same ones as a Quarto document, so they are kept once, beside the formats.
const QUARTO_OPTIONS = "quarto-options";
const takesQuartoOptions = (format) => format === "markdown" || format === "quarto";

export function readQuartoOptions(scope) {
  try {
    const value = JSON.parse(storage()?.getItem(key(scope, QUARTO_OPTIONS)) || "null");
    if (value && typeof value === "object") return { profile: value.profile || null, parameters: value.parameters || {} };
  } catch { /* unreadable: the defaults */ }
  return { profile: null, parameters: {} };
}

export function writeQuartoOptions(scope, { profile, parameters } = {}) {
  const value = { profile: profile || null, parameters: parameters || {} };
  try { storage()?.setItem(key(scope, QUARTO_OPTIONS), JSON.stringify(value)); } catch { /* private mode/quota */ }
  return value;
}

function withQuartoOptions(scope, format, preference) {
  return takesQuartoOptions(format) ? { ...preference, ...readQuartoOptions(scope) } : preference;
}

export function read(scope, format = "") {
  return withQuartoOptions(scope, format, stored(scope, format));
}

function stored(scope, format) {
  const fallback = defaults(format);
  try {
    const raw = storage()?.getItem(key(scope, format));
    if (!raw) return fallback;
    const value = JSON.parse(raw);
    if (!value || typeof value !== "object") return fallback;
    const allowed = ["automatic", "tool"];
    if (!allowed.includes(value.selection)) return fallback;
    if (value.backend && !["auto", "browser", "local"].includes(value.backend)) return fallback;
    const result = { ...fallback, ...value, format: format || value.format || "", output: "html" };
    // LaTeX has no local builder. A stored preference from another format, or
    // from an older build of the app, is read back as the browser engine
    // rather than as a choice the menu can no longer offer.
    if (format === "latex") {
      result.backend = result.backend === "local" ? "browser" : result.backend;
      if (result.tool !== "tex") { result.tool = "tex"; }
    }
    // Nor has Typst: the local choice is Calepin.
    if (format === "typst" && result.backend === "local" && result.tool === "typst") result.backend = "browser";
    return result;
  } catch { return fallback; }
}

function write(scope, format, next) {
  const value = { ...next };
  delete value.user;
  // The output is the visit's, not the browser's: what is stored leaves it
  // out, and what is handed back keeps it, because the caller is about to
  // render with the choice that was just made. The Quarto options have their
  // own record.
  const kept = { ...value };
  delete kept.output;
  delete kept.profile;
  delete kept.parameters;
  try { storage()?.setItem(key(scope, format), JSON.stringify(kept)); } catch { /* private mode/quota */ }
  return withQuartoOptions(scope, format, value);
}

export function update(scope, format, patch) {
  const current = read(scope, format);
  const changingTool = patch.selection === "tool" && patch.tool && (patch.tool !== current.tool || patch.backend !== current.backend);
  const next = { ...(changingTool || patch.selection === "automatic" ? defaults(format) : current), ...patch, format };
  if (next.selection === "automatic") {
    delete next.tool;
    delete next.engine;
    next.backend = "auto";
  }
  return write(scope, format, next);
}
