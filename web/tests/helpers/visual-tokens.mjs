// Shared by the golden writer (tools/visual-golden.mjs) and the golden test
// (tests/unit/visual-tree-tokens.mjs): the corpus, the languages, and one
// serialization so both sides compare the same bytes.
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { latex } from "../../src/lib/visual-markup/languages/latex/index.ts";
import { typst } from "../../src/lib/visual-markup/languages/typst/index.ts";
import { markdown } from "../../src/lib/visual-markup/languages/markdown/index.ts";

/** The repository root, which corpus paths are relative to. */
export const REPO_ROOT = resolve(fileURLToPath(new URL("../../../", import.meta.url)));
export const CORPUS_FILE = fileURLToPath(new URL("../fixtures/visual-tokens/corpus.json", import.meta.url));
export const GOLDEN_DIR = fileURLToPath(new URL("../fixtures/visual-tokens/golden/", import.meta.url));

export const LANGUAGES = { latex, typst, markdown };

/** Corpus entries: { name, language, path }. Names must be unique and filesystem safe. */
export function loadCorpus() {
  const corpus = JSON.parse(readFileSync(CORPUS_FILE, "utf8"));
  const names = new Set();
  for (const entry of corpus) {
    if (!/^[A-Za-z0-9_.-]+$/.test(entry.name)) throw new Error(`corpus name is not filesystem safe: ${entry.name}`);
    if (names.has(entry.name)) throw new Error(`corpus name is not unique: ${entry.name}`);
    if (!(entry.language in LANGUAGES)) throw new Error(`corpus language unknown: ${entry.language} (${entry.path})`);
    names.add(entry.name);
  }
  return corpus;
}

/** The source text of a corpus entry. */
export function readCorpusSource(entry) {
  return readFileSync(join(REPO_ROOT, entry.path), "utf8");
}

export function goldenPath(entry) {
  return join(GOLDEN_DIR, `${entry.name}.json`);
}

/** Entries filtered by VISUAL_TOKENS_ONLY=latex|typst|markdown when it is set. */
export function selectedEntries(corpus, only = process.env.VISUAL_TOKENS_ONLY) {
  if (!only) return corpus;
  if (!(only in LANGUAGES)) throw new Error(`VISUAL_TOKENS_ONLY must be latex, typst or markdown, not ${only}`);
  return corpus.filter(entry => entry.language === only);
}

// Objects get their keys sorted and undefined fields dropped, so the output
// does not depend on the order a producer built a token in.
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    const out = {};
    for (const key of Object.keys(value).sort()) {
      if (value[key] !== undefined) out[key] = canonical(value[key]);
    }
    return out;
  }
  return value;
}

/** The canonical JSON text of a token list, as written to the golden files. */
export function serializeTokens(tokens) {
  return `${JSON.stringify(canonical(tokens), null, 2)}\n`;
}

/** The canonical value of a token list, for comparison without the text. */
export function canonicalTokens(tokens) {
  return canonical(tokens);
}

export function readGolden(entry) {
  const file = goldenPath(entry);
  if (!existsSync(file)) return null;
  return readFileSync(file, "utf8");
}
