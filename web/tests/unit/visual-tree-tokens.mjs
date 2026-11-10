// Golden test for the visual layer's tokens: tokenizeSource() on every corpus
// file must match its golden, the reviewed snapshot of the tree-based tokens.
// After an intended change to the tokens, regenerate the goldens with
// tools/visual-golden.mjs and review the diff. VISUAL_TOKENS_ONLY=latex|typst|markdown
// limits the run.
import assert from "node:assert/strict";
import { test } from "node:test";
import { loadTypstSyntaxForTests } from "../helpers/typst-syntax.mjs";
import { LANGUAGES, canonicalTokens, loadCorpus, readCorpusSource, readGolden, selectedEntries, serializeTokens } from "../helpers/visual-tokens.mjs";
import { tokenizeSource } from "../../src/lib/visual-markup/core/tokenize.ts";

const entries = selectedEntries(loadCorpus());

// Where two token lists first differ, as a path such as "3.children.1" plus
// the differing token without its children. Null when they are equal.
function firstDifference(expected, actual, path = []) {
  const at = path.length ? path.join(".") : "(root)";
  if (JSON.stringify(expected) === JSON.stringify(actual)) return null;
  if (!Array.isArray(expected) || !Array.isArray(actual)) {
    return { path: at, expected: JSON.stringify(expected), actual: JSON.stringify(actual) };
  }
  for (let i = 0; i < Math.max(expected.length, actual.length); i++) {
    const e = expected[i];
    const a = actual[i];
    const next = [...path, i];
    if (e === undefined || a === undefined) {
      return { path: next.join("."), expected: JSON.stringify(withoutChildren(e)), actual: JSON.stringify(withoutChildren(a)) };
    }
    if (JSON.stringify(e) === JSON.stringify(a)) continue;
    if (JSON.stringify(withoutChildren(e)) !== JSON.stringify(withoutChildren(a))) {
      return { path: next.join("."), expected: JSON.stringify(withoutChildren(e)), actual: JSON.stringify(withoutChildren(a)) };
    }
    const inner = firstDifference(e.children ?? [], a.children ?? [], [...next, "children"]);
    if (inner) return inner;
  }
  return null;
}

function withoutChildren(token) {
  if (!token || typeof token !== "object") return token;
  const { children, ...rest } = token;
  return rest;
}

if (entries.some(entry => entry.language === "typst")) {
  await loadTypstSyntaxForTests();
}

for (const entry of entries) {
  test(`visual tokens: ${entry.path}`, () => {
    const golden = readGolden(entry);
    assert.notEqual(golden, null, `no golden for ${entry.name}; run tools/visual-golden.mjs`);
    const source = readCorpusSource(entry);
    const actual = tokenizeSource(LANGUAGES[entry.language], source);
    assert.notEqual(actual, null, `${entry.path}: tokenizeSource returned null (tree not loaded)`);
    if (serializeTokens(actual) === golden) return;

    const expected = JSON.parse(golden);
    const difference = firstDifference(expected, canonicalTokens(actual));
    const detail = difference
      ? `first difference at token ${difference.path}\n  expected: ${difference.expected}\n  actual:   ${difference.actual}`
      : "same tokens, different serialization";
    assert.fail(`${entry.path} (${entry.language}): ${detail}`);
  });
}
