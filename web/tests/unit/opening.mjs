// The opening rules, held to the corpus the server's `opening.rs` also reads,
// and the preview target the rules produce for the file open in the source.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { includersOf, openingFile, previewTarget, rendersAlone } from "../../src/lib/opening.js";

const corpus = JSON.parse(
  readFileSync(new URL("../fixtures/opening.json", import.meta.url), "utf8"),
);

for (const testCase of corpus.cases) {
  const texts = testCase.files;
  assert.equal(openingFile(texts), testCase.opening, `${testCase.name}: opening`);

  const includers = includersOf(texts);
  assert.deepEqual(
    Object.fromEntries(includers),
    testCase.includers,
    `${testCase.name}: includers`,
  );

  for (const path of Object.keys(texts)) {
    assert.equal(
      rendersAlone(path, texts, includers),
      testCase.alone.includes(path),
      `${testCase.name}: ${path} renders alone`,
    );
  }
}

const named = (name) => corpus.cases.find((testCase) => testCase.name === name).files;

const thesis = named("LaTeX thesis with chapters");
assert.deepEqual(
  previewTarget({ open: "chapters/intro.tex", locked: null, texts: thesis }),
  { path: "thesis.tex", reason: "inside", includers: ["thesis.tex"], choices: [] },
  "a thesis chapter previews inside the thesis",
);
assert.deepEqual(
  previewTarget({ open: "thesis.tex", locked: null, texts: thesis }),
  { path: "thesis.tex", reason: "alone", includers: [], choices: [] },
  "a thesis previews itself",
);

const shared = named("two papers share macros");
assert.deepEqual(
  previewTarget({ open: "macros.tex", locked: null, texts: shared }),
  { path: null, reason: "none", includers: ["a.tex", "b.tex"], choices: ["a.tex", "b.tex"] },
  "macros shared by two papers have no preview and list both",
);

const book = named("Typst book with chapters and a template");
assert.deepEqual(
  previewTarget({ open: "chapters/two.typ", locked: null, texts: book }),
  { path: "book.typ", reason: "inside", includers: ["chapters/one.typ"], choices: [] },
  "a nested chapter walks up to the book",
);

const papers = named("two papers share macros");
assert.deepEqual(
  previewTarget({ open: "macros.tex", locked: "b.tex", texts: papers }),
  { path: "b.tex", reason: "locked", includers: ["a.tex", "b.tex"], choices: [] },
  "a locked file wins over the open one",
);
assert.equal(
  previewTarget({ open: "a.tex", locked: "macros.tex", texts: papers }).reason,
  "alone",
  "a lock on a file that cannot render alone is ignored",
);

const paper = named("one LaTeX paper");
assert.deepEqual(
  previewTarget({ open: "refs.bib", locked: null, texts: paper }),
  { path: null, reason: "none", includers: [], choices: [] },
  "a bib file has no preview",
);

const cycle = {
  "a.typ": '#include "b.typ"\n',
  "b.typ": '#include "a.typ"\n',
};
assert.equal(
  previewTarget({ open: "a.typ", locked: null, texts: cycle }).reason,
  "none",
  "a cycle ends the walk",
);

const nested = {
  "shared.tex": "\\newcommand{\\x}{x}\n",
  "chapter.tex": "\\input{shared}\n",
  "a.tex": "\\documentclass{article}\n\\input{chapter}\n",
  "b.tex": "\\documentclass{article}\n\\input{chapter}\n",
};
const nestedTarget = previewTarget({ open: "shared.tex", locked: null, texts: nested });
assert.equal(nestedTarget.reason, "none", "a file under two includers has no single preview");
assert.deepEqual(nestedTarget.choices, ["a.tex", "b.tex"], "choices walk up past a file that cannot render alone");

const macros = {
  "macros.tex": "\\newcommand{\\x}{x}\n",
  "p1.tex": "\\documentclass{article}\n\\input{macros}\n",
  "p2.tex": "\\documentclass{article}\n\\input{macros}\n",
};
assert.deepEqual(
  previewTarget({ open: "macros.tex", locked: null, texts: macros }).choices,
  ["p1.tex", "p2.tex"],
  "macros included directly by two papers offer both",
);

console.log("opening: all checks passed");
