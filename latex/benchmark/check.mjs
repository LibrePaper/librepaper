import assert from "node:assert/strict";
import { CASES, selectedCases, treeOf, treeDigest } from "./corpus.mjs";
import { textAgreement, warnings } from "./results.mjs";

assert.equal(textAgreement("A complete bibliography", "A complete bibliography"), 1);
assert.ok(textAgreement("A complete bibliography", "A") < 0.6);
assert.equal(textAgreement("", ""), null);
assert.equal(warnings("Package biblatex Info: ... file 'biblatex-dm.cfg' not found.").length, 0);
assert.equal(warnings("Missing character: There is no α in font cmr10!").length, 1);
assert.equal(warnings("LaTeX Warning: There were undefined references.").length, 1);
assert.throws(() => selectedCases(["--case", "typo"]), /Unknown case/);
assert.equal(selectedCases(["--case", "multifile,packages"]).length, 2);

for (const example of CASES) {
  const tree = treeOf(example);
  assert.ok(tree.texts[tree.main]);
  assert.equal(treeDigest(tree), treeDigest(treeOf(example)));
  assert.ok(!Object.keys(tree.texts).some((p) => /(^|\/)logs\//.test(p)));
  assert.ok(!Object.keys(tree.assets).includes("main.pdf"));
}
const paper = treeOf(CASES.find((c) => c.id === "multifile"));
assert.ok(paper.texts["chapters/01.tex"]);
assert.ok(Object.keys(paper.assets).some((p) => p.endsWith(".pdf")), "retain PDF figures while excluding generated output");
const changed = structuredClone(paper);
changed.texts[changed.main] += "\n% edited";
assert.notEqual(treeDigest(changed), treeDigest(paper));
console.log(`benchmark: ${CASES.length} prepared cases; content identity and incomplete-output checks passed`);
