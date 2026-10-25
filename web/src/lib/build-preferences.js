// Per-browser build preferences: one per format for each person in this
// browser, shared by every document of that format. These values never enter
// collaborative state.
export const PREFIX = "librepaper-build-v3";
const ANONYMOUS_KEY = `${PREFIX}:anonymous-id`;
let anonymousId;

// `selection` is the LaTeX engine choice and `backend` is where the format
// renders.
const DEFAULTS = Object.freeze({ selection: "automatic", backend: "browser", output: "" });

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
export function defaults(format = "") { return { ...DEFAULTS, format, output: "html", backend: backendOf(format, "browser") }; }

// Where each format renders: in this browser or, in the desktop app, on this
// computer with the command set in Settings. Quarto has no browser engine of
// its own, so it always asks for this computer and Run code decides.
export const LOCAL_FORMATS = Object.freeze(["latex", "typst", "markdown", "quarto"]);

function backendOf(format, backend) {
  if (format === "quarto") return "local";
  return backend === "local" && LOCAL_FORMATS.includes(format) ? "local" : "browser";
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
  const output = readDefaultPreview(scope, format);
  const fallback = { ...defaults(format), output };
  try {
    const raw = storage()?.getItem(key(scope, format));
    if (!raw) return fallback;
    const value = JSON.parse(raw);
    if (!value || typeof value !== "object") return fallback;
    if (!["automatic", "tool"].includes(value.selection)) return fallback;
    delete value.tool;
    return { ...fallback, ...value, format: format || value.format || "", output, backend: backendOf(format, value.backend) };
  } catch { return fallback; }
}

function write(scope, format, next) {
  const value = { ...next, backend: backendOf(format, next.backend) };
  delete value.user;
  // The output is the View menu's choice for this visit: what is stored leaves
  // it out, and what is handed back keeps it, because the caller is about to
  // render with the choice that was just made. The default preview has its
  // own record.
  const kept = { ...value };
  delete kept.output;
  try { storage()?.setItem(key(scope, format), JSON.stringify(kept)); } catch { /* private mode/quota */ }
  return value;
}

export function update(scope, format, patch) {
  const current = read(scope, format);
  const base = patch.selection === "automatic" ? { ...defaults(format), backend: current.backend, output: current.output } : current;
  const next = { ...base, ...patch, format };
  if (next.selection === "automatic") delete next.engine;
  return write(scope, format, next);
}
