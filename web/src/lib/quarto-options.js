// Parsing and remembering the explicit choices made by the Quarto render
// controls.  This module deliberately has no rendering or execution hooks:
// its output is just data that a caller may put into a local job request.

const FORMATS = new Set(["default", "html", "pdf", "docx", "revealjs"]);
const PROFILE_RE = /^[A-Za-z0-9._-]+$/;
const PARAMETER_NAME_RE = /^[A-Za-z_][A-Za-z0-9_.-]*$/;
const MAX_PROFILE_BYTES = 128;
const MAX_PARAMETER_COUNT = 128;
const MAX_PARAMETER_NAME_BYTES = 128;
const MAX_PARAMETER_VALUE_BYTES = 16 * 1024;
const MAX_DOCUMENT_BYTES = 256;
const MAX_ORIGIN_BYTES = 2048;
const MAX_STORED_BYTES = 512 * 1024;
const STORAGE_PREFIX = "librepaper-quarto-render-options-v1";

const encoder = new TextEncoder();
const byteLength = (value) => encoder.encode(String(value)).byteLength;

export const DEFAULT_RENDER_OPTIONS = Object.freeze({
  format: "default",
  profile: "",
  parameters: Object.freeze({}),
});

function emptyOptions() {
  return { format: "default", profile: "", parameters: {} };
}

function invalid(message) {
  throw new Error(`Invalid Quarto render options: ${message}`);
}

function parseFormat(value) {
  if (value == null || value === "") return "default";
  if (typeof value !== "string" || !FORMATS.has(value)) {
    invalid("format must be default, html, pdf, docx, or revealjs");
  }
  return value;
}

function parseProfile(value) {
  if (value == null) return "";
  if (typeof value !== "string") invalid("profile must be a string");
  const profile = value.trim();
  if (!profile) return "";
  if (byteLength(profile) > MAX_PROFILE_BYTES || !PROFILE_RE.test(profile)) {
    invalid("profile must contain only ASCII letters, digits, dot, underscore, or hyphen");
  }
  return profile;
}

function parametersObject(value) {
  if (value == null) return {};
  if (typeof value === "string") {
    if (!value.trim()) return {};
    try {
      value = JSON.parse(value);
    } catch {
      invalid("parameters must be valid JSON");
    }
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    invalid("parameters must be a JSON object");
  }
  return value;
}

function parseParameters(value) {
  const source = parametersObject(value);
  const keys = Object.keys(source);
  if (keys.length > MAX_PARAMETER_COUNT) invalid("at most 128 parameters are allowed");

  const parameters = {};
  for (const key of keys) {
    if (byteLength(key) < 1 || byteLength(key) > MAX_PARAMETER_NAME_BYTES || !PARAMETER_NAME_RE.test(key)) {
      invalid(`parameter name is invalid: ${key}`);
    }
    const parameter = source[key];
    if (parameter !== null && !["string", "boolean", "number"].includes(typeof parameter)) {
      invalid(`parameter must be a scalar JSON value: ${key}`);
    }
    if (typeof parameter === "number" &&
        (!Number.isFinite(parameter) || Object.is(parameter, -0) ||
         (Number.isInteger(parameter) && !Number.isSafeInteger(parameter)))) {
      invalid(`parameter number is outside the portable range: ${key}`);
    }
    const serialized = typeof parameter === "string" ? parameter : JSON.stringify(parameter);
    if (byteLength(serialized) > MAX_PARAMETER_VALUE_BYTES) {
      invalid(`parameter is too large: ${key}`);
    }
    // Assignment to `__proto__` changes an ordinary object's prototype. Use
    // a data property so every valid JSON key remains an inert scalar value.
    Object.defineProperty(parameters, key, {
      value: parameter, enumerable: true, configurable: true, writable: true,
    });
  }
  return parameters;
}

/**
 * Parse explicit UI values into the scalar options accepted by the local
 * Quarto protocol. An omitted format stays "default" so the caller can
 * resolve a document's format; this function intentionally does not inspect
 * a document.
 * `parameters` may be the JSON text from a textarea or an already parsed
 * object, which also makes saved preferences easy to validate on load.
 */
export function parseRenderOptions({ format = null, profile = null, parameters = null, parametersText } = {}) {
  const parameterInput = parameters === null && parametersText !== undefined ? parametersText : parameters;
  return {
    format: parseFormat(format),
    profile: parseProfile(profile),
    parameters: parseParameters(parameterInput),
  };
}

function scopeParts(scope, origin, storage) {
  if (scope && typeof scope === "object" && !Array.isArray(scope)) {
    return {
      documentId: scope.documentId ?? scope.document ?? scope.slug,
      origin: scope.origin,
      storage: scope.storage,
    };
  }
  return { documentId: scope, origin, storage };
}

function scopeKey(scope, origin) {
  const documentId = String(scope ?? "");
  const resolvedOrigin = String(origin ?? globalThis.location?.origin ?? "");
  if (!documentId || byteLength(documentId) > MAX_DOCUMENT_BYTES) return null;
  if (!resolvedOrigin || byteLength(resolvedOrigin) > MAX_ORIGIN_BYTES) return null;
  return `${STORAGE_PREFIX}:${encodeURIComponent(resolvedOrigin)}:${encodeURIComponent(documentId)}`;
}

function storageFor(storage) {
  if (storage !== undefined) return storage;
  try { return globalThis.localStorage; } catch { return null; }
}

function storageArgs(scope, origin, storage) {
  const parts = scopeParts(scope, origin, storage);
  return { key: scopeKey(parts.documentId, parts.origin), storage: storageFor(parts.storage) };
}

/** Return the remembered options, or empty options when storage is unavailable/corrupt. */
export function loadRenderOptions(scope, origin, storage) {
  const { key, storage: target } = storageArgs(scope, origin, storage);
  if (!key || !target || typeof target.getItem !== "function") return emptyOptions();
  try {
    const raw = target.getItem(key);
    if (raw == null || typeof raw !== "string" || byteLength(raw) > MAX_STORED_BYTES) return emptyOptions();
    const record = JSON.parse(raw);
    if (record?.version !== 1 || !record.options || typeof record.options !== "object") return emptyOptions();
    return parseRenderOptions(record.options);
  } catch {
    return emptyOptions();
  }
}

/** Save options under a key scoped to both the current origin and document. */
export function saveRenderOptions(scope, origin, options, storage) {
  // Object form is convenient for browser callers: saveRenderOptions({
  // documentId, origin, options, storage }). Positional arguments remain
  // useful in small integrations and tests.
  let parts;
  if (scope && typeof scope === "object" && !Array.isArray(scope)) {
    parts = scopeParts(scope);
    options = scope.options ?? options;
  } else if (options === undefined && origin && typeof origin === "object" && !Array.isArray(origin)) {
    // Compact form used by the editor: saveRenderOptions(documentId, options).
    options = origin;
    parts = scopeParts(scope);
  } else {
    parts = scopeParts(scope, origin, storage);
  }
  const { key, storage: target } = storageArgs(parts.documentId, parts.origin, parts.storage);
  if (!key || !target || typeof target.setItem !== "function") return false;
  const parsed = parseRenderOptions(options || {});
  const raw = JSON.stringify({ version: 1, options: parsed });
  if (byteLength(raw) > MAX_STORED_BYTES) return false;
  try {
    target.setItem(key, raw);
    return true;
  } catch {
    return false;
  }
}

/** Remove remembered options for one document and origin. */
export function clearRenderOptions(scope, origin, storage) {
  const { key, storage: target } = storageArgs(scope, origin, storage);
  if (!key || !target || typeof target.removeItem !== "function") return false;
  try {
    target.removeItem(key);
    return true;
  } catch {
    return false;
  }
}

export function renderOptionsStorageKey(scope, origin) {
  const parts = scopeParts(scope, origin);
  return scopeKey(parts.documentId, parts.origin);
}
