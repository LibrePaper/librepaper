// livePatch: which edits can be shown by patching the rendered preview, and
// what the patch says. Everything doubtful must come back null.
import assert from "node:assert/strict";
import { livePatch, paragraphText } from "../../src/lib/live-preview/patch.js";

// A main file whose body starts on line 3.
const doc = (body) => `\\begin{document}\n\n${body}\n\n\\end{document}\n`;
const edit = (oldBody, newBody) =>
  livePatch({ path: "main.tex", oldText: doc(oldBody), newText: doc(newBody), isMain: true });

// Typing a letter in the middle of a word.
assert.deepEqual(edit("The quick brown fox jumps.", "The quicxk brown fox jumps."), {
  kind: "prose", file: "main.tex", line: 3,
  before: "The quic", deleted: "", inserted: "x", after: "k brown fox jumps.",
});

// Deleting a word.
assert.deepEqual(edit("The quick brown fox jumps.", "The quick fox jumps."), {
  kind: "prose", file: "main.tex", line: 3,
  before: "The quick ", deleted: "brown ", inserted: "", after: "fox jumps.",
});

// Inside \emph and \section.
assert.deepEqual(edit("A \\emph{significant} dog.", "A \\emph{significcant} dog."), {
  kind: "prose", file: "main.tex", line: 3,
  before: "signific", deleted: "", inserted: "c", after: "ant",
});
assert.deepEqual(edit("\\section{Introduction}", "\\section{Introductions}"), {
  kind: "prose", file: "main.tex", line: 3,
  before: "Introduction", deleted: "", inserted: "s", after: "",
});

// Not in the body of the main file.
assert.equal(
  livePatch({
    path: "main.tex", isMain: true,
    oldText: "\\documentclass{article}\n\nStray words here\n\\begin{document}\n\nHi.\n\n\\end{document}\n",
    newText: "\\documentclass{article}\n\nStray wordss here\n\\begin{document}\n\nHi.\n\n\\end{document}\n",
  }),
  null,
);

// A file that is not the main one: the top level is body.
const part = livePatch({
  path: "chapter.tex", isMain: false,
  oldText: "Plain words here and there.\n", newText: "Plain wordss here and there.\n",
});
assert.equal(part.kind, "prose");
assert.equal(part.inserted, "s");
assert.equal(part.before, "Plain words");

// Arguments of other commands.
assert.equal(edit("See \\label{sec:intro} here", "See \\label{sec:intros} here"), null);
assert.equal(edit("See \\cite{smith2020} here", "See \\cite{smith2021} here"), null);
assert.equal(edit("\\includegraphics{figure1}", "\\includegraphics{figure12}"), null);

// A comment.
assert.equal(edit("Some text % a comment about words\nNext.", "Some text % a comment aboutt words\nNext."), null);

// A backslash or a brace is never plain.
assert.equal(edit("The quick brown fox", "The qu\\ick brown fox"), null);
assert.equal(edit("The quick brown fox", "The qu{ick brown fox"), null);

// Apostrophe becomes a typographic one.
assert.deepEqual(edit("We dont know it well.", "We don't know it well."), {
  kind: "prose", file: "main.tex", line: 3,
  before: "We don", deleted: "", inserted: "\u2019", after: "t know it well.",
});

// A dash ligature made by the edit.
assert.equal(edit("See pages 10-20 here.", "See pages 10--20 here."), null);

// Inline math.
assert.deepEqual(edit("Let $x^2$ be.", "Let $x^3$ be."), {
  kind: "math", file: "main.tex", line: 3, oldTex: "x^2", tex: "x^3", display: false,
});

// Equation environment, label removed.
assert.deepEqual(
  edit("\\begin{equation}\\label{e}a+b\\end{equation}", "\\begin{equation}\\label{e}a+bc\\end{equation}"),
  { kind: "math", file: "main.tex", line: 3, oldTex: "a+b", tex: "a+bc", display: true },
);

// Other math environments, and an edit that adds a dollar.
assert.equal(edit("\\begin{align}a&=b\\end{align}", "\\begin{align}a&=bc\\end{align}"), null);
assert.equal(edit("Let $x^2$ be.", "Let $x^$2$ be."), null);

// Not a .tex file.
assert.equal(
  livePatch({ path: "main.typ", isMain: true, oldText: doc("The quick brown fox"), newText: doc("The quicxk brown fox") }),
  null,
);

// No change.
assert.equal(edit("The quick brown fox", "The quick brown fox"), null);

// The line is the line of the edit in the old text.
const lines = (last) => `First line here.\nSecond line here.\nThird line here.\nFourth line here.\n${last}`;
const seventh = edit(lines("Fifth line here."), lines("Fifth linee here."));
assert.equal(seventh.kind, "prose");
assert.equal(seventh.line, 7);

console.log("live-preview-patch: ok");

// The 32-character window can reach a blank line: no leading or trailing space.
{
  const para = "Welcome to the first paragraph of this text, eee fff.";
  const starts = [
    (body) => `\\begin{document}\n\n\\end{figure}\n\n${body}\n\n\\end{document}\n`,
    (body) => `\n${body}\n\n`,
  ];
  for (const wrap of starts) {
    const isMain = wrap === starts[0];
    for (let n = 1; n <= 26; n++) {
      const oldBody = para.slice(0, n) + para.slice(n + 1);
      const newBody = para;
      const patch = livePatch({ path: isMain ? "main.tex" : "a.tex", oldText: wrap(oldBody), newText: wrap(newBody), isMain });
      if (!patch) continue;
      assert.ok(!/^\s/.test(patch.before), `before has no leading space at ${n}`);
      assert.ok(!/\s$/.test(patch.after), `after has no trailing space at ${n}`);
      assert.equal(patch.before, para.slice(0, n), "before is the paragraph text before the caret");
    }
  }
  // The window after the edit reaches the following blank line.
  const short = "abc defg hij klmn";
  const patch = livePatch({
    path: "a.tex", isMain: false,
    oldText: `\n${short.slice(0, -1)}\n\nNext.\n`, newText: `\n${short}\n\nNext.\n`,
  });
  assert.ok(patch);
  assert.equal(patch.after, "");
  assert.equal(patch.before, short.slice(0, -1));
}

// paragraphText: the paragraph around an offset of the new text.
{
  const para = (text, needle) => paragraphText({ path: "main.tex", text, at: text.indexOf(needle) + needle.length, isMain: true });
  // A new plain paragraph after another one.
  assert.deepEqual(para(doc("First one.\n\nSecond one"), "Second one"), { startLine: 5, endLine: 5, text: "Second one" });
  // A command in the paragraph: refused.
  assert.equal(para(doc("First one.\n\nSecond \\cite{x} one"), "Second"), null);
  // The preamble: refused.
  assert.equal(paragraphText({ path: "main.tex", text: "Some words\n\\begin{document}\n\n\\end{document}\n", at: 5, isMain: true }), null);
  // Several lines collapse to one.
  assert.deepEqual(para(doc("First one.\n\nSecond   line\nthird line"), "third"), { startLine: 5, endLine: 6, text: "Second line third line" });
  // The apostrophe is rendered.
  assert.equal(para(doc("First one.\n\nIt's here"), "It's").text, "It\u2019s here");
  // Not a tex file, and a blank line.
  assert.equal(paragraphText({ path: "a.md", text: "words", at: 2, isMain: false }), null);
  assert.equal(para(doc("First one.\n\n\nx"), "First one.\n"), null);
}
