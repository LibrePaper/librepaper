import assert from "node:assert/strict";
import { numberLabels } from "../../src/lib/visual-markup/labels.js";

// Sections: nesting, a starred section that does not advance, and the
// subsubsection under a subsection.
{
  const labels = numberLabels({
    "main.tex": [
      "\\begin{document}",
      "\\section{Intro}\\label{sec:intro}",
      "\\subsection{A}\\label{sec:a}",
      "\\section*{Notes}",
      "\\section{Method}\\label{sec:m}",
      "\\subsection{Data}",
      "\\subsubsection{Deep}\\label{sec:deep}",
      "\\end{document}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("sec:intro"), { number: "1", kind: "section" });
  assert.deepEqual(labels.get("sec:a"), { number: "1.1", kind: "section" });
  assert.deepEqual(labels.get("sec:m"), { number: "2", kind: "section" });
  assert.deepEqual(labels.get("sec:deep"), { number: "2.1.1", kind: "section" });
}

// Text before \begin{document} is not walked.
{
  const labels = numberLabels({
    "main.tex": "\\section{Ghost}\\label{sec:ghost}\n\\begin{document}\n\\section{Real}\\label{sec:real}\n\\end{document}",
  }, "main.tex");
  assert.equal(labels.has("sec:ghost"), false);
  assert.deepEqual(labels.get("sec:real"), { number: "1", kind: "section" });
}

// \appendix switches the section letters to A, B, and subsections follow.
{
  const labels = numberLabels({
    "main.tex": [
      "\\section{Body}\\label{sec:body}",
      "\\appendix",
      "\\section{Proofs}\\label{sec:proofs}",
      "\\subsection{Lemma}\\label{sec:lemma}",
      "\\section{Tables}\\label{sec:tables}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("sec:body"), { number: "1", kind: "section" });
  assert.deepEqual(labels.get("sec:proofs"), { number: "A", kind: "section" });
  assert.deepEqual(labels.get("sec:lemma"), { number: "A.1", kind: "section" });
  assert.deepEqual(labels.get("sec:tables"), { number: "B", kind: "section" });
}

// An align of three rows: the middle row has \nonumber, so the third row is 2.
{
  const labels = numberLabels({
    "main.tex": [
      "\\begin{align}",
      "a &= b \\label{eq:one} \\\\",
      "c &= d \\nonumber \\\\",
      "e &= f \\label{eq:three}",
      "\\end{align}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("eq:one"), { number: "1", kind: "equation" });
  assert.deepEqual(labels.get("eq:three"), { number: "2", kind: "equation" });
}

// \tag sets the number without advancing the counter.
{
  const labels = numberLabels({
    "main.tex": [
      "\\begin{equation}\\tag{A}\\label{eq:tagged}",
      "x = 1",
      "\\end{equation}",
      "\\begin{equation}\\label{eq:next}",
      "y = 2",
      "\\end{equation}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("eq:tagged"), { number: "A", kind: "equation" });
  assert.deepEqual(labels.get("eq:next"), { number: "1", kind: "equation" });
}

// A starred equation* is unnumbered and does not advance the counter.
{
  const labels = numberLabels({
    "main.tex": [
      "\\begin{equation*}",
      "x = 1",
      "\\end{equation*}",
      "\\begin{equation}\\label{eq:n}",
      "y = 2",
      "\\end{equation}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("eq:n"), { number: "1", kind: "equation" });
}

// Figures and tables are numbered by their captions, each with its own counter.
{
  const labels = numberLabels({
    "main.tex": [
      "\\begin{figure}",
      "\\caption{Plot}\\label{fig:plot}",
      "\\end{figure}",
      "\\begin{table}",
      "\\caption{Data}\\label{tab:data}",
      "\\end{table}",
      "\\begin{figure}",
      "\\caption{Second}\\label{fig:two}",
      "\\end{figure}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("fig:plot"), { number: "1", kind: "figure" });
  assert.deepEqual(labels.get("tab:data"), { number: "1", kind: "table" });
  assert.deepEqual(labels.get("fig:two"), { number: "2", kind: "figure" });
}

// Inside a float, a label binds to the most recent numbered thing in scope:
// before the caption that is the enclosing section, after it the caption.
{
  const labels = numberLabels({
    "main.tex": [
      "\\section{Setup}",
      "\\begin{figure}",
      "\\label{fig:before}",
      "\\caption{Early}",
      "\\label{fig:after}",
      "\\end{figure}",
    ].join("\n"),
  }, "main.tex");
  assert.deepEqual(labels.get("fig:before"), { number: "1", kind: "section" });
  assert.deepEqual(labels.get("fig:after"), { number: "1", kind: "figure" });
}

// A label in a comment is ignored.
{
  const labels = numberLabels({
    "main.tex": "% \\label{sec:hidden}\n\\section{Real}\\label{sec:real}",
  }, "main.tex");
  assert.equal(labels.has("sec:hidden"), false);
  assert.deepEqual(labels.get("sec:real"), { number: "1", kind: "section" });
}

// \input is inlined at its position, and its labels number in order.
{
  const labels = numberLabels({
    "main.tex": [
      "\\begin{document}",
      "\\section{Start}\\label{sec:start}",
      "\\input{sections/intro}",
      "\\section{End}\\label{sec:end}",
      "\\end{document}",
    ].join("\n"),
    "sections/intro.tex": "\\section{Intro}\\label{sec:intro}\n\\subsection{Detail}\\label{sec:detail}\n",
  }, "main.tex");
  assert.deepEqual(labels.get("sec:start"), { number: "1", kind: "section" });
  assert.deepEqual(labels.get("sec:intro"), { number: "2", kind: "section" });
  assert.deepEqual(labels.get("sec:detail"), { number: "2.1", kind: "section" });
  assert.deepEqual(labels.get("sec:end"), { number: "3", kind: "section" });
}

// An input resolves against the main file's folder, with .tex added.
{
  const labels = numberLabels({
    "paper/main.tex": "\\begin{document}\n\\input{sections/intro}\n\\end{document}",
    "paper/sections/intro.tex": "\\section{Intro}\\label{sec:intro}",
  }, "paper/main.tex");
  assert.deepEqual(labels.get("sec:intro"), { number: "1", kind: "section" });
}

// A missing input is ignored.
{
  const labels = numberLabels({
    "main.tex": "\\section{A}\\label{sec:a}\n\\input{nowhere}\n\\section{B}\\label{sec:b}",
  }, "main.tex");
  assert.deepEqual(labels.get("sec:b"), { number: "2", kind: "section" });
}

// Include cycles terminate: a file that inputs main, and a file that inputs itself.
{
  const labels = numberLabels({
    "main.tex": "\\section{A}\\label{sec:a}\n\\input{other}",
    "other.tex": "\\section{B}\\label{sec:b}\n\\input{main}",
  }, "main.tex");
  assert.equal(labels.has("sec:a"), true);
  assert.deepEqual(labels.get("sec:b"), { number: "2", kind: "section" });

  const self = numberLabels({
    "main.tex": "\\section{A}\\label{sec:a}\n\\input{main}",
  }, "main.tex");
  assert.deepEqual(self.get("sec:a"), { number: "1", kind: "section" });
}
