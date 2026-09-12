// Per-browser build preferences. These values never enter collaborative state.
const PREFIX = "librepaper-build-v2";
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
function key({ origin = globalThis.location?.origin || "", user = "anonymous", document = "" } = {}) {
  return `${PREFIX}:${JSON.stringify([String(origin), String(user || "anonymous"), String(document)])}`;
}

export function defaults(format = "") { return { ...DEFAULTS, format, output: ["latex", "typst"].includes(format) ? "pdf" : "html" }; }

export function read(scope, format = "") {
  const fallback = defaults(format);
  try {
    const raw = storage()?.getItem(key(scope));
    if (!raw) return fallback;
    const value = JSON.parse(raw);
    if (!value || typeof value !== "object") return fallback;
    const allowed = ["automatic", "tool"];
    if (!allowed.includes(value.selection)) return fallback;
    if (value.backend && !["auto", "browser", "local"].includes(value.backend)) return fallback;
    return { ...fallback, ...value, format: format || value.format || "" };
  } catch { return fallback; }
}

export function write(scope, next) {
  const value = { ...next };
  delete value.user;
  try { storage()?.setItem(key(scope), JSON.stringify(value)); } catch { /* private mode/quota */ }
  return value;
}

export function update(scope, format, patch) {
  const current = read(scope, format);
  const changingTool = patch.selection === "tool" && patch.tool && (patch.tool !== current.tool || patch.backend !== current.backend || patch.preset !== current.preset);
  const next = { ...(changingTool || patch.selection === "automatic" ? defaults(format) : current), ...patch, format };
  if (next.selection === "automatic") {
    delete next.tool;
    delete next.engine;
    delete next.preset;
    next.backend = "auto";
  }
  return write(scope, next);
}

export function storageKey(scope) { return key(scope); }
