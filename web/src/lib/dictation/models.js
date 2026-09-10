// The dictation model catalog: what the settings panel offers, and what the
// recognizer worker needs to fetch and run each entry. Pure and browser-free
// so `web/checks/dictation-models.mjs` drives it under Node, the same
// discipline as `web/src/lib/latex/local.js`. See SPEC-dictation.md 4.5 and
// `docs/dictation-interfaces.md`.
//
// Revisions are pinned by hand, in the spirit of `wasm-modules.lock`: moving
// one is a commit that says why, not a routine dependency bump.

// The 99 language codes Whisper was trained on (BCP 47 primary subtags).
const WHISPER_LANGUAGES = Object.freeze([
  "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr",
  "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi",
  "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no",
  "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk",
  "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk",
  "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw",
  "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc",
  "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
  "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl",
  "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
]);

// The 25 European languages of Parakeet v3 (BCP 47 primary subtags).
const PARAKEET_LANGUAGES = Object.freeze([
  "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de",
  "el", "hu", "it", "lv", "lt", "mt", "pl", "pt", "ro", "sk",
  "sl", "es", "sv", "ru", "uk",
]);

export const MODELS = Object.freeze([
  {
    id: "whisper-base",
    label: "Whisper base",
    kind: "local",
    repo: "onnx-community/whisper-base",
    revision: "1846881b6b3a3024392c1eea3ad983695bc23925",
    dtype: "q8",
    sizeBytes: 60_000_000,
    languages: WHISPER_LANGUAGES,
    acceptsLanguage: true,
    punctuates: true,
    capitalizes: true,
    description: "The light option. 99 languages.",
  },
  {
    id: "whisper-small",
    label: "Whisper small",
    kind: "local",
    repo: "onnx-community/whisper-small",
    revision: "36050c46d777d46dc4b5f43f6d90574fc38f8732",
    dtype: "q8",
    sizeBytes: 250_000_000,
    languages: WHISPER_LANGUAGES,
    acceptsLanguage: true,
    punctuates: true,
    capitalizes: true,
    description: "The default. 99 languages.",
  },
  {
    id: "parakeet-ctc",
    label: "Parakeet CTC 0.6B",
    kind: "local",
    repo: "onnx-community/parakeet-ctc-0.6b-ONNX",
    revision: "7df2cab7aed886b8b7f80d68a8214007e4847601",
    dtype: "q8",
    sizeBytes: 600_000_000,
    languages: PARAKEET_LANGUAGES,
    acceptsLanguage: false,
    punctuates: true,
    capitalizes: true,
    description: "Best accuracy on its languages. No language option.",
  },
  {
    id: "browser",
    label: "Browser built-in",
    kind: "browser",
    sizeBytes: 0,
    languages: "any",
    acceptsLanguage: true,
    punctuates: true,
    capitalizes: true,
    description: "No download. Audio may be sent to the browser vendor.",
  },
]);

export const DEFAULT_MODEL = "whisper-small";

// The voice activity detector the worker loads alongside every local model.
export const VAD = Object.freeze({
  repo: "onnx-community/silero-vad",
  revision: "e71cae966052b992a7eca6b17738916ce0eca4ec",
});

export function modelById(id) {
  return MODELS.find((model) => model.id === id);
}

function primarySubtag(tag) {
  return String(tag).split("-")[0].toLowerCase();
}

// BCP 47 primary subtag match; "any" (the browser backend) matches everything.
export function supportsLanguage(model, tag) {
  if (!model || !tag) return false;
  if (model.languages === "any") return true;
  const primary = primarySubtag(tag);
  return model.languages.includes(primary);
}

// SPEC 4.9: a setting other than "auto" wins when the model supports it;
// otherwise the first navigator language the model supports; otherwise null
// (the recognizer's own auto-detection, when it has one).
export function pickLanguage(model, setting, navigatorLanguages) {
  if (setting && setting !== "auto" && supportsLanguage(model, setting)) {
    return setting;
  }
  for (const tag of navigatorLanguages || []) {
    if (supportsLanguage(model, tag)) return tag;
  }
  return null;
}
