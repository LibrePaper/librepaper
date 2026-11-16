// Snapshot updater for the golden token files of the visual-markup corpus. It
// writes the tokens tokenizeSource() gives today. Run it after an intended change
// to the visual tokens, then review the golden diff (git diff of
// web/tests/fixtures/visual-tokens/golden): a change you did not mean to make
// shows there. It does not decide what is right; the reviewed diff does.
//
// Run from web/: node --experimental-transform-types --import ./tests/helpers/ts-resolve.mjs tools/visual-golden.mjs
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tokenizeSource } from "../src/lib/visual-markup/core/tokenize.ts";
import { loadTypstSyntaxForTests } from "../tests/helpers/typst-syntax.mjs";
import { GOLDEN_DIR, LANGUAGES, goldenPath, loadCorpus, readCorpusSource, serializeTokens } from "../tests/helpers/visual-tokens.mjs";

// The Typst tokens are read from its syntax tree, so the module must be loaded first.
await loadTypstSyntaxForTests();

const corpus = loadCorpus();
mkdirSync(GOLDEN_DIR, { recursive: true });

// Goldens of files that left the corpus go too, so the directory matches it.
const wanted = new Set(corpus.map(entry => `${entry.name}.json`));
for (const file of readdirSync(GOLDEN_DIR)) {
  if (!wanted.has(file)) rmSync(join(GOLDEN_DIR, file));
}

for (const entry of corpus) {
  const source = readCorpusSource(entry);
  const tokens = tokenizeSource(LANGUAGES[entry.language], source);
  if (!tokens) throw new Error(`${entry.name}: no tokens; the syntax tree is not ready`);
  writeFileSync(goldenPath(entry), serializeTokens(tokens));
  console.log(`${entry.name}: ${tokens.length} top-level tokens`);
}
console.log(`wrote ${corpus.length} golden files to ${GOLDEN_DIR}`);
