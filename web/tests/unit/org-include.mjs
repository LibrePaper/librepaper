// #+INCLUDE expanded on the text side: nesting, relative paths, cycles, missing
// files, the :lines and :minlevel options, and the src, example and export forms.
import assert from "node:assert/strict";
import { test } from "node:test";
import { expandOrgIncludes } from "../../src/lib/org-include.js";

const expand = (main, texts) => expandOrgIncludes({ main, texts, assets: {} }).texts[main];

test("a simple include is replaced by the file's text", () => {
  const out = expand("main.org", {
    "main.org": "* Intro\nHello.\n#+INCLUDE: \"part.org\"\n* End\n",
    "part.org": "* Part\nBody.\n",
  });
  assert.equal(out, "* Intro\nHello.\n* Part\nBody.\n* End\n");
});

test("an include inside an included file is expanded too", () => {
  const out = expand("main.org", {
    "main.org": "#+INCLUDE: \"a.org\"\n",
    "a.org": "A.\n#+INCLUDE: \"b.org\"\n",
    "b.org": "B.\n",
  });
  assert.equal(out, "A.\nB.\n");
});

test("relative paths resolve from the including file's folder", () => {
  const out = expand("paper/main.org", {
    "paper/main.org": "#+INCLUDE: \"sections/a.org\"\n",
    "paper/sections/a.org": "A.\n#+INCLUDE: \"figs.org\"\n#+INCLUDE: \"../shared.org\"\n",
    "paper/sections/figs.org": "Figs.\n",
    "paper/shared.org": "Shared.\n",
  });
  assert.equal(out, "A.\nFigs.\nShared.\n");
});

test("a file that includes itself, directly or through another, is skipped", () => {
  const out = expand("main.org", {
    "main.org": "Top.\n#+INCLUDE: \"a.org\"\n",
    "a.org": "A.\n#+INCLUDE: \"b.org\"\n",
    "b.org": "B.\n#+INCLUDE: \"a.org\"\n",
  });
  assert.equal(out, "Top.\nA.\nB.\n");
  assert.equal(expand("main.org", { "main.org": "#+INCLUDE: \"main.org\"\nX.\n" }), "X.\n");
});

test("a missing file leaves its include line as it is", () => {
  const source = "Before.\n#+INCLUDE: \"gone.org\"\nAfter.\n";
  assert.equal(expand("main.org", { "main.org": source }), source);
});

test(":lines keeps the 1-based inclusive range", () => {
  const part = Array.from({ length: 12 }, (_, i) => `line ${i + 1}`).join("\n") + "\n";
  assert.equal(
    expand("main.org", { "main.org": "#+INCLUDE: \"p.txt\" :lines \"3-5\"\n", "p.txt": part }),
    "line 3\nline 4\nline 5\n",
  );
  assert.equal(
    expand("main.org", { "main.org": "#+INCLUDE: \"p.txt\" :lines \"11-\"\n", "p.txt": part }),
    "line 11\nline 12\n",
  );
  assert.equal(
    expand("main.org", { "main.org": "#+INCLUDE: \"p.txt\" :lines \"-2\"\n", "p.txt": part }),
    "line 1\nline 2\n",
  );
});

test(":minlevel shifts the shallowest headline to that level and keeps the depth below it", () => {
  const out = expand("main.org", {
    "main.org": "#+INCLUDE: \"part.org\" :minlevel 2\n",
    "part.org": "* Top\n** Sub\n*** Deeper\n",
  });
  assert.equal(out, "** Top\n*** Sub\n**** Deeper\n");
  // A shallower minimum moves headlines up, never past one star.
  const up = expand("main.org", {
    "main.org": "#+INCLUDE: \"part.org\" :minlevel 1\n",
    "part.org": "*** Deep\n**** Deeper\n",
  });
  assert.equal(up, "* Deep\n** Deeper\n");
});

test("a src include wraps the file in a source block and does not expand it", () => {
  const out = expand("main.org", {
    "main.org": "#+INCLUDE: \"code/run.py\" src python\n",
    "code/run.py": "#+INCLUDE: \"x.org\"\nprint(1)\n",
  });
  assert.equal(out, "#+begin_src python\n#+INCLUDE: \"x.org\"\nprint(1)\n#+end_src\n");
});

test("an example include escapes lines that Org would read as structure", () => {
  const out = expand("main.org", {
    "main.org": "#+INCLUDE: \"notes.txt\" example\n",
    "notes.txt": "* not a headline\nplain\n",
  });
  assert.equal(out, "#+begin_example\n,* not a headline\nplain\n#+end_example\n");
});

test("an export include wraps the file in an export block for its backend", () => {
  const out = expand("main.org", {
    "main.org": "#+INCLUDE: \"macros.tex\" export latex\n",
    "macros.tex": "\\newcommand{\\R}{\\mathbb{R}}\n",
  });
  assert.equal(out, "#+begin_export latex\n\\newcommand{\\R}{\\mathbb{R}}\n#+end_export\n");
});

test("a text with no include is returned unchanged, and other texts are kept", () => {
  const texts = { "main.org": "* Plain\n", "other.org": "* Other\n" };
  const tree = expandOrgIncludes({ main: "main.org", texts, assets: {} });
  assert.equal(tree.texts["main.org"], "* Plain\n");
  assert.equal(tree.texts["other.org"], "* Other\n");
});
