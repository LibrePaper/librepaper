// Dictation's localStorage keys (SPEC-dictation.md 4.10;
// docs/dictation-interfaces.md "settings.js").
//
// `storage` is passed in rather than read from the global so the service and
// its checks can supply `null` (storage disabled or unavailable) or a fake.
// Every read tolerates a missing store, a store that throws (Safari private
// mode, quota), and -- for the one JSON-shaped key -- malformed content left
// behind by a future version or a hand-edited value: none of that should
// ever throw back into the caller, just fall back to the default.
export const KEYS = Object.freeze({
  backend: "librepaper-dictation-backend",
  model: "librepaper-dictation-model",
  language: "librepaper-dictation-language",
  confirmed: "librepaper-dictation-confirmed",
});

const DEFAULTS = Object.freeze({
  backend: "local",
  model: "whisper-small",
  language: "auto",
});

function readRaw(storage, key) {
  if (!storage) return null;
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

function writeRaw(storage, key, value) {
  if (!storage) return;
  try {
    storage.setItem(key, value);
  } catch {
    /* quota or a disabled store; not fatal */
  }
}

function readConfirmedList(storage) {
  const raw = readRaw(storage, KEYS.confirmed);
  if (!raw) return [];
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.filter((id) => typeof id === "string");
}

/// -> { backend, model, language, confirmed: [] }, each defaulted
/// independently so a partial or corrupted store still yields a usable
/// settings object.
export function readSettings(storage) {
  return {
    backend: readRaw(storage, KEYS.backend) || DEFAULTS.backend,
    model: readRaw(storage, KEYS.model) || DEFAULTS.model,
    language: readRaw(storage, KEYS.language) || DEFAULTS.language,
    confirmed: readConfirmedList(storage),
  };
}

/// Writes only the keys present in `patch`; `confirmed`, if given, replaces
/// the whole list (use `confirm()` to add one entry).
export function writeSettings(storage, patch) {
  if (patch.backend !== undefined) writeRaw(storage, KEYS.backend, patch.backend);
  if (patch.model !== undefined) writeRaw(storage, KEYS.model, patch.model);
  if (patch.language !== undefined) writeRaw(storage, KEYS.language, patch.language);
  if (patch.confirmed !== undefined) writeRaw(storage, KEYS.confirmed, JSON.stringify(patch.confirmed));
}

export function isConfirmed(storage, modelId) {
  return readConfirmedList(storage).includes(modelId);
}

export function confirm(storage, modelId) {
  const list = readConfirmedList(storage);
  if (!list.includes(modelId)) list.push(modelId);
  writeRaw(storage, KEYS.confirmed, JSON.stringify(list));
}
