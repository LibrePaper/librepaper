// Writes the golden token files for the visual-markup corpus. The goldens are
// made with the old regex tokenizer on purpose: tokenizeSource() uses a
// language's tree-based tokenize() once it has one, and the goldens must keep
// recording the rules the tree walks are checked against.
//
// Run from web/: node --experimental-transform-types --import ./tests/helpers/ts-resolve.mjs tools/visual-golden.mjs
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Tokenizer } from "../src/lib/visual-markup/core/tokenizer.ts";
import { GOLDEN_DIR, LANGUAGES, goldenPath, loadCorpus, readCorpusSource, serializeTokens } from "../tests/helpers/visual-tokens.mjs";

const corpus = loadCorpus();
mkdirSync(GOLDEN_DIR, { recursive: true });

// Goldens of files that left the corpus go too, so the directory matches it.
const wanted = new Set(corpus.map(entry => `${entry.name}.json`));
for (const file of readdirSync(GOLDEN_DIR)) {
  if (!wanted.has(file)) rmSync(join(GOLDEN_DIR, file));
}

for (const entry of corpus) {
  const source = readCorpusSource(entry);
  const tokens = new Tokenizer(LANGUAGES[entry.language]).tokenize(source);
  writeFileSync(goldenPath(entry), serializeTokens(tokens));
  console.log(`${entry.name}: ${tokens.length} top-level tokens`);
}
console.log(`wrote ${corpus.length} golden files to ${GOLDEN_DIR}`);
