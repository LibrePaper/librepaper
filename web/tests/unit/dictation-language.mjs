import { whisperLanguage } from "../../src/lib/dictation/language.js";
import assert from "node:assert/strict";
import { modelById, pickLanguage } from "../../src/lib/dictation/models.js";
import { whisper_language_to_code } from "../../node_modules/@huggingface/transformers/src/models/whisper/common_whisper.js";

const cases = [["fr-CA", "fr"], ["EN-us", "en"], ["de", "de"], [null, null], ["", null]];
for (const [input, expected] of cases) {
  const actual = whisperLanguage(input);
  if (actual !== expected) {
    console.error(`dictation-language: ${JSON.stringify(input)} -> ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`);
    process.exit(1);
  }
}
for (const locale of ["en-US", "fr-CA"]) {
  const picked = pickLanguage(modelById("whisper-small"), "auto", [locale]);
  assert.throws(() => whisper_language_to_code(picked), /not supported/);
  assert.equal(pickLanguage(modelById("browser"), "auto", [locale]), locale);
  const normalized = whisperLanguage(picked);
  try {
    whisper_language_to_code(normalized);
  } catch (error) {
    console.error(`dictation-language: production Whisper validator rejected ${locale} -> ${normalized}: ${error.message}`);
    process.exit(1);
  }
}
console.log("dictation-language: BCP 47 tags normalize to Whisper language codes");
