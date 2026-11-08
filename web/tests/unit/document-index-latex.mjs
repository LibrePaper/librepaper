// The LaTeX adapter of the document index: equivalence with the existing
// scanners on realistic sources, a unit case per scanner, and the project
// queries run over the adapter's output for a two-file project.
import assert from "node:assert/strict";
import test from "node:test";
import { extractOutline } from "../../src/lib/outline.js";
import { citedKeys } from "../../src/lib/citation-keys.js";
import { skipRanges } from "../../src/lib/spellcheck/index.js";
import { latexAdapter } from "../../src/lib/document-index/adapters/latex.js";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { definition, diagnostics, rename } from "../../src/lib/document-index/queries.js";

/** Range of the first `needle` at or after `from`. */
function at(text, needle, from = 0) {
  const start = text.indexOf(needle, from);
  assert.notEqual(start, -1, `missing ${needle}`);
  return [start, start + needle.length];
}

/** The items of one kind. */
function kindOf(items, kind) {
  return items.filter((item) => item.kind === kind);
}

/** The text a range covers. */
function textOf(text, range) {
  return text.slice(range[0], range[1]);
}

/** Items sorted by start, then end, so order does not matter in comparisons. */
function byRange(items) {
  return [...items].sort((a, b) => a.range[0] - b.range[0] || a.range[1] - b.range[1]);
}

const PAPER = String.raw`\documentclass{article}
\usepackage{graphicx}
\addbibresource[style=authoryear]{refs.bib}
\newcommand{\R}{\mathbb{R}}
\DeclareMathOperator{\argmax}{arg\,max}
\begin{document}
\section{Introduction}\label{sec:intro}
We study \cite{smith2020} and \cref{fig:plot,eq:main}.
\input{intro}
\begin{figure}
  \centering
  \includegraphics[width=0.5\linewidth]{plot.png}
  \caption{Results of the   main
    experiment}
  \label{fig:plot}
\end{figure}
% \label{fake:comment} \ref{fake:ref} \cite{commented}
\section{Model}
\begin{equation}\label{eq:main}
  y = \beta x + \varepsilon
\end{equation}
The cost is $c > 0$ and \[ \argmax_x f \] holds, \$5 is not math.
$$\R^n$$
\begin{verbatim}
\section{Fake}
\includegraphics{fake.png}
$not math
\end{verbatim}
\bibliography{refs,more}
\end{document}
`;

const NOTES = String.raw`\documentclass{report}
\addbibresource[style=numeric]{refs.bib}
\begin{document}
\chapter{Background}
\include{chapters/one}
\section{Data}\label{sec:data}
Table~\ref{tab:sizes} lists sizes; see \autoref{sec:data} and \citet{doe1999}.
\begin{table}
  \caption{Sizes of  the samples}\label{tab:sizes}
  \begin{tabular}{ll}
    a & b \\
  \end{tabular}
\end{table}
\subsection*{Proofs}
\begin{align}
  a &= b \label{eq:a}\\
  c &= d
\end{align}
\begin{itemize}
  \item One $x_1$
  \begin{enumerate}
    \item Two
  \end{enumerate}
\end{itemize}
\begin{lstlisting}
  % not a comment \section{Fake}
\end{lstlisting}
\includegraphics[width=\linewidth]{figs/plot.pdf}
\bibliography{refs}
\end{document}
`;

const INTRO = String.raw`\section{Intro}\label{fig:b}
Text with \cite{jones}.
`;

// The label names of each fixture, sorted. `% \label{fake:comment}` is in a
// comment and is not a label.
const EXPECTED_LABELS = {
  paper: ["eq:main", "fig:plot", "sec:intro"],
  notes: ["eq:a", "sec:data", "tab:sizes"],
  intro: ["fig:b"],
};

for (const [name, text] of [["paper", PAPER], ["notes", NOTES], ["intro", INTRO]]) {
  test(`latex adapter agrees with the scanners on ${name}`, () => {
    const items = latexAdapter(text, `${name}.tex`);

    assert.deepEqual(
      byRange(kindOf(items, "heading")).map((item) => ({ title: item.title, level: item.level, from: item.range[0] })),
      extractOutline(text, "latex").map((heading) => ({ title: heading.title, level: heading.level, from: heading.from })),
    );

    assert.deepEqual(
      byRange(kindOf(items, "cite")).map((item) => ({ key: item.name, from: item.range[0], to: item.range[1] })),
      citedKeys(text, "latex").map((cite) => ({ key: cite.key, from: cite.from, to: cite.to })),
    );

    assert.deepEqual(
      byRange(kindOf(items, "markup")).map((item) => item.range),
      skipRanges(text, "latex"),
    );

    // The label names were read from the fixtures by hand. The old label
    // scanner that used to be the oracle here has been removed.
    const labelNames = [...new Set(kindOf(items, "label").map((item) => item.name))].sort();
    assert.deepEqual(labelNames, EXPECTED_LABELS[name]);
  });
}

test("labels take the caption of their innermost environment", () => {
  const text = String.raw`\begin{figure}
\caption[Short]{Results of the
  main   run}
\label{fig:a}
\end{figure}
`;
  const [label] = kindOf(latexAdapter(text, "main.tex"), "label");
  assert.equal(label.name, "fig:a");
  assert.equal(label.title, "Results of the main run");
  assert.equal(textOf(text, label.range), String.raw`\label{fig:a}`);
  assert.equal(textOf(text, label.nameRange), "fig:a");
});

test("a label without its own caption takes the nearest heading title", () => {
  const text = String.raw`\section{Methods}
\label{sec:m}
`;
  const [label] = kindOf(latexAdapter(text, "main.tex"), "label");
  assert.equal(label.title, "Methods");
});

test("a begin or end between the heading and the label drops the heading title", () => {
  const text = String.raw`\section{Methods}
\begin{itemize}
\item a
\end{itemize}
\label{sec:m}
`;
  const [label] = kindOf(latexAdapter(text, "main.tex"), "label");
  assert.equal("title" in label, false);
});

test("a caption in a nested environment does not title a label outside it", () => {
  const text = String.raw`\section{Intro}
\begin{figure}
\begin{center}
\caption{Inner}
\end{center}
\label{fig:x}
\end{figure}
`;
  const [label] = kindOf(latexAdapter(text, "main.tex"), "label");
  assert.equal(label.name, "fig:x");
  assert.equal("title" in label, false);
});

test("a label with no environment and no heading has no title", () => {
  const [label] = kindOf(latexAdapter(String.raw`\label{x}`, "main.tex"), "label");
  assert.equal("title" in label, false);
});

test("references split comma lists, with trimmed name spans", () => {
  const text = String.raw`See \eqref{eq:1}, \pageref{p}, and \cref{a, b}.`;
  const refs = kindOf(latexAdapter(text, "main.tex"), "ref");
  assert.deepEqual(refs.map((ref) => ref.name), ["eq:1", "p", "a", "b"]);
  assert.ok(refs.every((ref) => ref.namespace === "label"));
  assert.deepEqual(refs[0].range, at(text, String.raw`\eqref{eq:1}`));
  assert.equal(textOf(text, refs[0].nameRange), "eq:1");
  // Both keys of \cref{a, b} share the whole command as their range.
  assert.deepEqual(refs[2].range, refs[3].range);
  assert.equal(textOf(text, refs[2].range), String.raw`\cref{a, b}`);
  assert.equal(textOf(text, refs[2].nameRange), "a");
  assert.equal(textOf(text, refs[3].nameRange), "b");
  assert.equal(refs[3].nameRange[0], at(text, "b}")[0]);
});

test("commented, escaped and verbatim content is read correctly", () => {
  const text = String.raw`% \label{c1}
50\% \label{esc}
x\\% \label{after}
\begin{verbatim}
\label{v} \section{S} $x
\end{verbatim}
\verb|\label{vb}| \label{real}
`;
  const items = latexAdapter(text, "main.tex");
  assert.deepEqual(kindOf(items, "label").map((item) => item.name), ["esc", "real"]);
  assert.deepEqual(kindOf(items, "heading"), []);
  assert.deepEqual(kindOf(items, "math"), []);
  // The verbatim body is a code item and a block, from the end of the \begin line to the \end.
  const code = kindOf(items, "code");
  assert.equal(code.length, 1);
  assert.equal(textOf(text, code[0].range), "\n\\label{v} \\section{S} $x\n");
});

test("include, asset and bibliography items", () => {
  const text = String.raw`\input{intro}
\include{ chapters/two }
\subfile{sub}
\includegraphics[width=2cm]{plot.png}
\bibliography{refs, more}
\addbibresource[style=x]{main.bib}
`;
  const items = latexAdapter(text, "main.tex");
  const includes = kindOf(items, "include");
  assert.deepEqual(includes.map((item) => item.name), ["intro", "chapters/two", "sub"]);
  assert.equal(textOf(text, includes[1].nameRange), "chapters/two");
  assert.equal(textOf(text, includes[0].range), String.raw`\input{intro}`);
  const assets = kindOf(items, "asset");
  assert.deepEqual(assets.map((item) => item.name), ["plot.png"]);
  assert.equal(textOf(text, assets[0].nameRange), "plot.png");
  const bibliography = kindOf(items, "bibliography");
  assert.deepEqual(bibliography.map((item) => item.name), ["refs", "more", "main.bib"]);
  assert.equal(textOf(text, bibliography[1].nameRange), "more");
  assert.equal(textOf(text, bibliography[2].nameRange), "main.bib");
});

test("nested environments are blocks, and single-line ones are not", () => {
  const text = String.raw`\begin{figure}
\begin{center}
x
\end{center}
\end{figure}
\begin{itemize}\item a\end{itemize}
`;
  const blocks = kindOf(latexAdapter(text, "main.tex"), "block");
  assert.deepEqual(blocks.map((block) => block.name).sort(), ["center", "figure"]);
  const figure = blocks.find((block) => block.name === "figure");
  assert.equal(textOf(text, figure.range), "\n\\begin{center}\nx\n\\end{center}\n");
  assert.deepEqual(figure.attrs, { environment: "figure" });
});

test("same-name nesting pairs each \\end with the nearest \\begin", () => {
  const text = String.raw`\begin{itemize}
\begin{itemize}
\item a
\end{itemize}
\end{itemize}
`;
  const blocks = kindOf(latexAdapter(text, "main.tex"), "block");
  assert.equal(blocks.length, 2);
  const [outer, inner] = byRange(blocks);
  assert.equal(textOf(text, outer.range), "\n\\begin{itemize}\n\\item a\n\\end{itemize}\n");
  assert.equal(textOf(text, inner.range), "\n\\item a\n");
});

test("a verbatim-like environment is a block and a code item", () => {
  const text = String.raw`\begin{lstlisting}
x = 1
\end{lstlisting}
`;
  const items = latexAdapter(text, "main.tex");
  const [block] = kindOf(items, "block");
  const [code] = kindOf(items, "code");
  assert.equal(block.name, "lstlisting");
  assert.deepEqual(code.range, block.range);
});

test("math environments, dollar, bracket and double-dollar spans", () => {
  const text = String.raw`\begin{equation}
x
\end{equation}
a $b$ c $$d$$ e \[f\] g \$5 h $\R$
`;
  const math = kindOf(latexAdapter(text, "main.tex"), "math").map((item) => textOf(text, item.range));
  assert.deepEqual(math, [
    String.raw`\begin{equation}
x
\end{equation}`,
    "$b$",
    "$$d$$",
    String.raw`\[f\]`,
    String.raw`$\R$`,
  ]);
});

test("definitions name each defining form", () => {
  const text = String.raw`\newcommand{\R}{\mathbb{R}}
\renewcommand\foo{x}
\providecommand{\bar}[1]{#1}
\DeclareMathOperator{\argmax}{arg\,max}
\def\baz{q}
\newenvironment{proof2}{}{}
`;
  const definitions = kindOf(latexAdapter(text, "main.tex"), "definition");
  assert.deepEqual(definitions.map((item) => item.name), ["R", "foo", "bar", "argmax", "baz", "proof2"]);
  for (const item of definitions) {
    assert.equal(textOf(text, item.nameRange), item.name);
  }
  assert.equal(textOf(text, definitions[0].range), String.raw`\newcommand{\R}`);
  assert.equal(textOf(text, definitions[1].range), String.raw`\renewcommand\foo`);
});

test("project queries work on adapter output for a two-file project", () => {
  const main = String.raw`\input{intro}
See \ref{fig:b} and \ref{fig:zz}.
`;
  const intro = String.raw`\section{Intro}\label{fig:b}
`;
  const project = createProjectIndex();
  project.update({ path: "main.tex", format: "latex", items: latexAdapter(main, "main.tex"), length: main.length });
  project.update({ path: "intro.tex", format: "latex", items: latexAdapter(intro, "intro.tex"), length: intro.length });
  project.setFiles(["main.tex", "intro.tex"]);

  const [refFrom] = at(main, "fig:b");
  const found = definition(project, "main.tex", refFrom + 1);
  assert.deepEqual(found, [{ path: "intro.tex", from: intro.indexOf("fig:b"), to: intro.indexOf("fig:b") + 5 }]);

  const problems = diagnostics(project, "main.tex");
  assert.deepEqual(problems.map((problem) => [problem.code, problem.name]), [["unknown-label", "fig:zz"]]);
  assert.deepEqual(diagnostics(project, "intro.tex"), []);

  const edits = rename(project, "intro.tex", intro.indexOf("fig:b") + 1, "fig:c");
  assert.deepEqual(edits.map((edit) => [edit.path, textOf(edit.path === "main.tex" ? main : intro, [edit.from, edit.to])]), [
    ["intro.tex", "fig:b"],
    ["main.tex", "fig:b"],
  ]);
});

test("labels in verbatim environments and \\verb spans are not indexed", () => {
  const text = String.raw`\begin{minted}{python}
\label{m1}
\end{minted}
\begin{comment}
\label{m2} \ref{m3}
\end{comment}
\verb*+\label{m4}+ \verb|\ref{m5}|
\label{real}
`;
  const items = latexAdapter(text, "main.tex");
  assert.deepEqual(kindOf(items, "label").map((item) => item.name), ["real"]);
  assert.deepEqual(kindOf(items, "ref"), []);
});

test("an escaped percent does not start a comment, a real one does", () => {
  const text = String.raw`rate 5\% \label{kept} and 5%
\label{gone}
\ref{shown} % \ref{hidden} \cite{hidden}
`;
  const items = latexAdapter(text, "main.tex");
  assert.deepEqual(kindOf(items, "label").map((item) => item.name), ["kept", "gone"]);
  assert.deepEqual(kindOf(items, "ref").map((item) => item.name), ["shown"]);
  assert.deepEqual(kindOf(items, "cite"), []);
});

test("a cite with an optional argument gives one item per key", () => {
  const text = String.raw`See \cite[p.~3]{a, b} and \citep*{c} and \nocite{*}.`;
  const cites = kindOf(latexAdapter(text, "main.tex"), "cite");
  assert.deepEqual(cites.map((cite) => cite.name), ["a", "b", "c"]);
  assert.deepEqual(cites.map((cite) => textOf(text, cite.nameRange)), ["a", "b", "c"]);
  assert.deepEqual(cites[0].nameRange, [text.indexOf("a,"), text.indexOf("a,") + 1]);
  assert.ok(cites.every((cite) => cite.namespace === "cite"));
});

test("a starred section still gives a heading and titles its label", () => {
  const text = String.raw`\section*{T}\label{sec:t}
`;
  const items = latexAdapter(text, "main.tex");
  assert.deepEqual(kindOf(items, "heading").map((item) => [item.title, item.level]), [["T", 3]]);
  assert.equal(kindOf(items, "label")[0].title, "T");
});

test("\\begin with a space before the brace is an environment", () => {
  const text = String.raw`\begin {figure}
\caption{Cap}
\label{fig:s}
\end {figure}
`;
  const items = latexAdapter(text, "main.tex");
  const [block] = kindOf(items, "block");
  assert.equal(block.name, "figure");
  assert.equal(kindOf(items, "label")[0].title, "Cap");
});

test("labels inside math environments and commands with a space before the brace are read", () => {
  const text = String.raw`\begin{align}
a &= b \label {eq:z}
\end{align}
\ref {eq:z}
`;
  const items = latexAdapter(text, "main.tex");
  assert.deepEqual(kindOf(items, "label").map((item) => item.name), ["eq:z"]);
  assert.deepEqual(kindOf(items, "ref").map((item) => item.name), ["eq:z"]);
  assert.equal(kindOf(items, "math").length, 1);
});

test("a realistic 350 KB document is indexed in under a second", () => {
  const text = PAPER.repeat(Math.ceil(350_000 / PAPER.length));
  assert.ok(text.length >= 350_000);
  const start = performance.now();
  const items = latexAdapter(text, "big.tex");
  const elapsed = performance.now() - start;
  assert.ok(items.length > 1000);
  assert.ok(elapsed < 1000, `took ${elapsed} ms`);
});

test("a caption keeps nested groups in its title", () => {
  const text = "\\begin{figure}\n\\caption{A \\emph{comparison}}\n\\label{fig:a}\n\\end{figure}\n";
  const [label] = kindOf(latexAdapter(text, "main.tex"), "label");
  assert.equal(label.name, "fig:a");
  assert.equal(label.title, "A \\emph{comparison}");
});

test("a cite finds its keys past an optional argument that holds a command", () => {
  const text = String.raw`See \cite[see][\S~3]{key} and \cite[p.~\textbf{2}]{other}.`;
  const cites = kindOf(latexAdapter(text, "main.tex"), "cite");
  assert.deepEqual(cites.map((cite) => cite.name), ["key", "other"]);
});

test("a caption title leaves out comments", () => {
  const text = "\\begin{figure}\n\\caption{A % note\ncomparison}\n\\label{fig:b}\n\\end{figure}\n";
  const [label] = kindOf(latexAdapter(text, "main.tex"), "label");
  assert.equal(label.name, "fig:b");
  assert.equal(label.title, "A comparison");
});

test("a long caption keeps its title and a long cite list keeps its keys", () => {
  const words = "word ".repeat(80).trim();
  const keys = Array.from({ length: 40 }, (_, k) => `key${k}`);
  const text = `\\begin{figure}\n\\caption{${words}}\n\\label{fig:c}\n\\end{figure}\n\\cite{${keys.join(", ")}}\n`;
  const items = latexAdapter(text, "main.tex");
  assert.equal(kindOf(items, "label")[0].title, words);
  assert.deepEqual(kindOf(items, "cite").map((cite) => cite.name), keys);
});

test("a comment before a ref or include target is not part of its name", () => {
  const text = "\\ref{% note\nsec:a}\n\\input{% chapter\nintro}\n";
  const items = latexAdapter(text, "main.tex");
  const [ref] = kindOf(items, "ref");
  assert.equal(ref.name, "sec:a");
  assert.equal(text.slice(...ref.nameRange), "sec:a");
  assert.equal(kindOf(items, "include")[0].name, "intro");
});
