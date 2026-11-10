import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { sourceLanguage } from "../../src/lib/source-language.js";

// closeBrackets() reads the language's closeBrackets language data. Typst and
// Org set their own; the Typst stream mode (used until the wasm parser loads)
// must match the parser's so the pairs do not change when it swaps in.
function brackets(path) {
  const state = EditorState.create({ doc: "", extensions: [sourceLanguage(path)] });
  return state.languageDataAt("closeBrackets", 0).flatMap((data) => data.brackets || []);
}

const typst = brackets("a.typ");
assert.ok(typst.includes("$"), "Typst pairs dollar signs for maths");
assert.ok(typst.includes('"'), "Typst pairs double quotes");
assert.ok(!typst.includes("'"), "Typst does not pair apostrophes, which are prose");

const org = brackets("a.org");
assert.ok(org.includes("("), "Org pairs parentheses");
assert.ok(org.includes('"'), "Org pairs double quotes");
assert.ok(!org.includes("'"), "Org does not pair apostrophes, which are prose");
assert.ok(!org.includes("$"), "Org does not pair dollar signs");
for (const markup of ["*", "/", "=", "~"]) {
  assert.ok(!org.includes(markup), `Org does not pair markup character ${markup}`);
}

assert.ok(brackets("a.tex").includes("$"), "LaTeX pairs dollar signs");
