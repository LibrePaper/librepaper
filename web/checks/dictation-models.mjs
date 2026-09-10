// Catalog shape and language-picking behavior for
// web/src/lib/dictation/models.js, plus a check that the recognizer worker's
// options table (web/src/lib/dictation/worker.js) has not silently dropped a
// catalog entry. See SPEC-dictation.md section 8 and
// docs/dictation-interfaces.md.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { MODELS, DEFAULT_MODEL, modelById, supportsLanguage, pickLanguage } from "../src/lib/dictation/models.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-models: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

const HEX40 = /^[0-9a-f]{40}$/;

/* --------------------------------------------------------- catalog shape */

const seenIds = new Set();
for (const model of MODELS) {
  check(`${model.id || "<no id>"} has an id`, typeof model.id === "string" && model.id.length > 0);
  check(`${model.id} has a label`, typeof model.label === "string" && model.label.length > 0);
  check(`${model.id} has a kind`, model.kind === "local" || model.kind === "browser");
  check(`${model.id} id is unique`, !seenIds.has(model.id), model.id);
  seenIds.add(model.id);

  check(`${model.id} sizeBytes is a number`, typeof model.sizeBytes === "number");
  const languagesOk = model.languages === "any" || (Array.isArray(model.languages) && model.languages.length > 0);
  check(`${model.id} has a non-empty languages array or "any"`, languagesOk, JSON.stringify(model.languages)?.slice(0, 60));
  for (const flag of ["acceptsLanguage", "punctuates", "capitalizes"]) {
    check(`${model.id}.${flag} is boolean`, typeof model[flag] === "boolean", typeof model[flag]);
  }

  if (model.kind === "local") {
    check(`${model.id} has a repo`, typeof model.repo === "string" && model.repo.length > 0);
    check(`${model.id} has a 40-hex revision`, HEX40.test(model.revision || ""), model.revision);
    check(`${model.id} has a dtype`, typeof model.dtype === "string" && model.dtype.length > 0);
    check(`${model.id} sizeBytes > 0`, model.sizeBytes > 0, String(model.sizeBytes));
  } else {
    check(`${model.id} (browser) sizeBytes is 0`, model.sizeBytes === 0, String(model.sizeBytes));
  }
}

check("DEFAULT_MODEL resolves", modelById(DEFAULT_MODEL) !== undefined, DEFAULT_MODEL);

/* --------------------------------------------------------- supportsLanguage / pickLanguage */

const whisper = modelById("whisper-small") || modelById("whisper-base");
const parakeet = modelById("parakeet-ctc");
const browser = MODELS.find((m) => m.kind === "browser");

check("a Whisper model exists to test against", whisper !== undefined);
check("a browser entry exists", browser !== undefined);

if (whisper) {
  check(
    "supportsLanguage matches on the BCP 47 primary subtag (fr-CA against Whisper)",
    supportsLanguage(whisper, "fr-CA") === true,
  );
  check(
    "pickLanguage honors a setting the model supports, even region-tagged",
    pickLanguage(whisper, "fr-CA", ["en"]) === "fr-CA",
  );
}

if (parakeet) {
  // Danish (da) is in Parakeet's 25 languages; Japanese (ja) is not.
  check("supportsLanguage is false for a language the model lacks", supportsLanguage(parakeet, "ja") === false);
  check(
    "pickLanguage falls through to navigator languages when the setting is unsupported",
    pickLanguage(parakeet, "ja", ["ja-JP", "da"]) === "da",
  );
  check(
    "pickLanguage returns null when nothing matches",
    pickLanguage(parakeet, "ja", ["ja-JP", "ko"]) === null,
  );
}

if (browser) {
  check('supportsLanguage("any") matches everything', supportsLanguage(browser, "xx-YY") === true);
  check('pickLanguage picks the setting outright for the "any" browser entry', pickLanguage(browser, "de", ["en"]) === "de");
  check(
    'pickLanguage falls back to the first navigator language for the "any" browser entry when "auto"',
    pickLanguage(browser, "auto", ["en-US", "fr"]) === "en-US",
  );
}

/* --------------------------------------------------------- worker options table coverage */

const here = dirname(fileURLToPath(import.meta.url));
const workerSource = readFileSync(resolve(here, "../src/lib/dictation/worker.js"), "utf8");
for (const model of MODELS) {
  if (model.kind !== "local") continue;
  check(
    `worker.js options table mentions catalog id "${model.id}"`,
    workerSource.includes(`"${model.id}"`),
  );
}

/* --------------------------------------------------------- run */

if (failures) {
  console.error(`dictation-models: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`dictation-models: ${MODELS.length} catalog entr${MODELS.length === 1 ? "y" : "ies"} checked`);
process.exit(0);
