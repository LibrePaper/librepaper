import assert from "node:assert/strict";
import { test } from "node:test";
import { EditorState } from "@codemirror/state";
import { foldable } from "@codemirror/language";
import { latexFolding } from "../../src/lib/latex-folding.js";

// The text a fold would hide when the cursor is on lineNumber, or null.
function folded(doc, lineNumber) {
  const state = EditorState.create({ doc, extensions: [latexFolding] });
  const line = state.doc.line(lineNumber);
  const range = foldable(state, line.from, line.to);
  return range ? doc.slice(range.from, range.to) : null;
}

test("an environment folds to the line before its closing \\end", () => {
  const doc = "\\begin{itemize}\n\\item a\n\\end{itemize}\n";
  assert.equal(folded(doc, 1), "\n\\item a\n");
});

test("nested environments of the same name match their own \\end", () => {
  const doc = "\\begin{itemize}\n\\begin{itemize}\n\\item a\n\\end{itemize}\n\\end{itemize}";
  assert.equal(folded(doc, 1), "\n\\begin{itemize}\n\\item a\n\\end{itemize}\n");
  assert.equal(folded(doc, 2), "\n\\item a\n");
});

test("an environment nested inside another of a different name is matched by name", () => {
  const doc = "\\begin{figure}\n\\begin{tabular}{c}\nx\n\\end{tabular}\n\\end{figure}";
  assert.equal(folded(doc, 1), "\n\\begin{tabular}{c}\nx\n\\end{tabular}\n");
  assert.equal(folded(doc, 2), "\nx\n");
});

test("an \\end after an unescaped % is a comment", () => {
  const doc = "\\begin{itemize}\n% \\end{itemize}\n\\item x\n\\end{itemize}";
  assert.equal(folded(doc, 1), "\n% \\end{itemize}\n\\item x\n");
});

test("an escaped \\% does not start a comment", () => {
  const doc = "\\begin{itemize}\n50\\% \\end{itemize}\n";
  assert.equal(folded(doc, 1), "\n50\\% ");
});

test("\\end inside a verbatim body is ignored", () => {
  const doc = "\\begin{verbatim}\n\\end{itemize}\n\\end{verbatim}\n";
  assert.equal(folded(doc, 1), "\n\\end{itemize}\n");

  const wrapped = "\\begin{itemize}\n\\begin{verbatim}\n\\end{itemize}\n\\end{verbatim}\n\\end{itemize}";
  assert.equal(folded(wrapped, 1), "\n\\begin{verbatim}\n\\end{itemize}\n\\end{verbatim}\n");
});

test("a section folds up to the line before the next section", () => {
  const doc = "\\section{A}\ntext\n\\subsection{B}\nmore\n\\section{C}\nend";
  assert.equal(folded(doc, 1), "\ntext\n\\subsection{B}\nmore");
});

test("a subsection stops at the next section of a higher level", () => {
  const doc = "\\section{A}\ntext\n\\subsection{B}\nmore\n\\section{C}\nend";
  assert.equal(folded(doc, 3), "\nmore");
});

test("a starred section still folds", () => {
  const doc = "\\section*{A}\ntext\n\\section*{B}\nend";
  assert.equal(folded(doc, 1), "\ntext");
});

test("the last section stops before \\end{document}", () => {
  const doc = "\\section{A}\ntext\n\\end{document}\n";
  assert.equal(folded(doc, 1), "\ntext");
});

test("a last section with no \\end{document} runs to the end of the document", () => {
  const doc = "\\section{A}\ntext";
  assert.equal(folded(doc, 1), "\ntext");
});

test("an unmatched \\begin gives no fold", () => {
  const doc = "\\begin{itemize}\n\\item a";
  assert.equal(folded(doc, 1), null);
});

test("a section with an empty body or a single line gives no fold", () => {
  assert.equal(folded("\\section{A}\n\\section{B}\nx", 1), null);
});

test("a \\begin inside a Verbatim body gives no fold", () => {
  const doc = "\\begin{Verbatim}\n\\begin{itemize}\nx\n\\end{Verbatim}\n";
  assert.equal(folded(doc, 2), null);
  assert.equal(folded(doc, 1), "\n\\begin{itemize}\nx\n");
});
