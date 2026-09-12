// Transformers.js' Whisper implementation accepts the model's primary
// language code. Browser and navigator language settings are BCP 47 tags
// (for example, `fr-CA`), so normalize only at the local recognizer boundary.
export function whisperLanguage(tag) {
  if (!tag) return null;
  const primary = String(tag).split("-")[0].toLowerCase();
  return primary || null;
}
