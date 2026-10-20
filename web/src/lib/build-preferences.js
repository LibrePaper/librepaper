// Per-browser build preferences: one per format for each person in this
// browser, shared by every document of that format. These values never enter
// collaborative state.
export const PREFIX = "librepaper-build-v3";
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

// HTML is the default for every format. Each person can set their own default
// per format in Settings > Rendering, kept in its own record. The View menu's
// choice lasts for the visit and is never written.
export function defaults(format = "") { return { ...DEFAULTS, format, output: "html" }; }

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

/** @param {unknown} scope @param {{profile?: string|null, parameters?: Record<string, unknown>}} [options] */
export function writeQuartoOptions(scope, { profile, parameters } = {}) {
  const value = { profile: profile || null, parameters: parameters || {} };
  try { storage()?.setItem(key(scope, QUARTO_OPTIONS), JSON.stringify(value)); } catch { /* private mode/quota */ }
  return value;
}

function withQuartoOptions(scope, format, preference) {
  return takesQuartoOptions(format) ? { ...preference, ...readQuartoOptions(scope) } : preference;
}

const defaultPreview = (format) => `default-preview:${format}`;

export function readDefaultPreview(scope, format = "") {
  try {
    return JSON.parse(storage()?.getItem(key(scope, defaultPreview(format))) || "null") === "pdf" ? "pdf" : "html";
  } catch { return "html"; }
}

export function writeDefaultPreview(scope, format, value) {
  const output = value === "pdf" ? "pdf" : "html";
  try { storage()?.setItem(key(scope, defaultPreview(format)), JSON.stringify(output)); } catch { /* private mode/quota */ }
  return read(scope, format);
}

export function read(scope, format = "") {
  return withQuartoOptions(scope, format, stored(scope, format));
}

function stored(scope, format) {
  const output = readDefaultPreview(scope, format);
  const fallback = { ...defaults(format), output };
  try {
    const raw = storage()?.getItem(key(scope, format));
    if (!raw) return fallback;
    const value = JSON.parse(raw);
    if (!value || typeof value !== "object") return fallback;
    const allowed = ["automatic", "tool"];
    if (!allowed.includes(value.selection)) return fallback;
    if (value.backend && !["auto", "browser", "local"].includes(value.backend)) return fallback;
    const result = { ...fallback, ...value, format: format || value.format || "", output };
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
  // The output is the View menu's choice for this visit: what is stored leaves
  // it out, and what is handed back keeps it, because the caller is about to
  // render with the choice that was just made. The default preview and the
  // Quarto options have their own records.
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
  const next = { ...(changingTool || patch.selection === "automatic" ? { ...defaults(format), output: current.output } : current), ...patch, format };
  if (next.selection === "automatic") {
    delete next.tool;
    delete next.engine;
    next.backend = "auto";
  }
  return write(scope, format, next);
}
