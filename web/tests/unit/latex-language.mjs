import assert from "node:assert/strict";
import { test } from "node:test";
import { EditorState } from "@codemirror/state";
import { ensureSyntaxTree, foldable } from "@codemirror/language";
import {
  autoCloseEnvironment, inMath, latex, latexLanguage,
} from "../../src/lib/latex-language/index.js";

const ARTICLE = String.raw`\documentclass[11pt]{article}
\usepackage[utf8]{inputenc}
\usepackage{amsmath}
% a comment with $ and \begin{itemize} in it
\newcommand{\R}{\mathbb{R}}
\begin{document}
\section{Intro}
Text with 100\% fun and \{braces\}, a break\\
next line. Inline $x^2$ and \(y\) too.
\begin{itemize}
\item one
\item[bold] two [not an option]
\end{itemize}
\begin{equation}
  E = mc^2 \label{eq:1}
\end{equation}
\begin{align*}
a &= b \\
c &= \frac{d}{e}
\end{align*}
\[ \int_0^1 x\,dx \]
$$ a + b $$
Code \verb|$| and \verb+\end{itemize}+ here.
\begin{verbatim}
$ not math \end{itemize} \section{x}
\end{verbatim}
\begin{lstlisting}[language=Python]
x = "$" # \end{itemize}
\end{lstlisting}
\begin{itemize}
\begin{enumerate}
\item nested
\end{enumerate}
\end{itemize}
\end{document}
`;

function parse(text) {
  return latexLanguage.parser.parse(text);
}

function nodes(tree, name) {
  const found = [];
  tree.iterate({ enter(node) { if (node.name === name) found.push({ from: node.from, to: node.to }); } });
  return found;
}

function errors(tree) {
  const found = [];
  tree.iterate({ enter(node) { if (node.type.isError) found.push(node.from); } });
  return found;
}

function stateFor(doc, cursor = doc.length) {
  const state = EditorState.create({ doc, selection: { anchor: cursor }, extensions: [latex()] });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

const at = (doc, text, nth = 0) => {
  let index = -1;
  for (let i = 0; i <= nth; i++) index = doc.indexOf(text, index + 1);
  assert.notEqual(index, -1, `${text} not found`);
  return index;
};

test("a realistic article parses without error nodes", () => {
  assert.deepEqual(errors(parse(ARTICLE)), []);
});

test("commands, optional arguments and sections are recognised", () => {
  const tree = parse(ARTICLE);
  assert.ok(nodes(tree, "Command").length > 10);
  assert.equal(nodes(tree, "OptionalArgument").length, 4); // [11pt] [utf8] [bold] [language=Python]
  assert.equal(nodes(tree, "SectionName").length, 1);
  // ] and [ in running text are plain text.
  const stray = at(ARTICLE, "[not an option]");
  assert.ok(!nodes(tree, "OptionalArgument").some((n) => n.from === stray));
});

test("inMath is true inside every math form and false elsewhere", () => {
  const state = stateFor(ARTICLE);
  const inside = (text, offset = 1, nth = 0) => inMath(state, at(ARTICLE, text, nth) + offset);
  assert.ok(inside("x^2"));
  assert.ok(inside("y\\) too", 0));
  assert.ok(inside("E = mc"));
  assert.ok(inside("a &= b"));
  assert.ok(inside("\\int"));
  assert.ok(inside("a + b"));
  assert.ok(inside("{d}", 1), "inside a group in math");

  assert.ok(!inside("Intro"));
  assert.ok(!inside("Text with"));
  assert.ok(!inside("100\\% fun"));
  assert.ok(!inside("\\item one"));
  assert.ok(!inside("|$|", 1), "inside \\verb");
  assert.ok(!inside("$ not math"));
  assert.ok(!inside("x = ", 5));
  assert.ok(!inside("a comment with $", 5));
  assert.ok(!inside("\\documentclass"));
  // Edges: before the opening delimiter and after the closing one are outside.
  const dollar = at(ARTICLE, "$x^2$");
  assert.ok(!inMath(state, dollar));
  assert.ok(inMath(state, dollar + 1));
  assert.ok(inMath(state, dollar + 4));
  assert.ok(!inMath(state, dollar + 5));
});

test("verbatim bodies hold no commands or math", () => {
  const tree = parse(ARTICLE);
  const bodies = nodes(tree, "VerbBody");
  assert.equal(bodies.length, 2);
  for (const body of bodies) {
    const inside = [];
    tree.iterate({
      from: body.from,
      to: body.to,
      enter(node) {
        if (node.from >= body.from && node.to <= body.to && node.type.name !== "VerbBody") inside.push(node.name);
      },
    });
    assert.deepEqual(inside.filter((n) => n !== "Document"), []);
  }
  const verbatim = ARTICLE.slice(bodies[0].from, bodies[0].to);
  assert.ok(verbatim.includes("$ not math \\end{itemize}"));
  assert.equal(nodes(tree, "Verb").length, 2);
});

test("a verbatim body ends only at its own \\end", () => {
  const doc = "\\begin{verbatim*}\n\\end{verbatim}\n\\end{verbatim*}\nafter $x$";
  const tree = parse(doc);
  assert.deepEqual(errors(tree), []);
  const [body] = nodes(tree, "VerbBody");
  assert.equal(doc.slice(body.from, body.to), "\n\\end{verbatim}\n");
  assert.equal(nodes(tree, "InlineMath").length, 1);
});

test("comments and escaped percent signs", () => {
  const doc = "50\\% of $x$ % trailing $ comment\nnext";
  const tree = parse(doc);
  assert.deepEqual(errors(tree), []);
  const [comment] = nodes(tree, "Comment");
  assert.equal(doc.slice(comment.from, comment.to), "% trailing $ comment");
  assert.equal(nodes(tree, "InlineMath").length, 1);
});

test("environments fold from the end of the begin line to before the end", () => {
  const doc = "\\begin{itemize}\n\\item a\n\\end{itemize}\n";
  const state = stateFor(doc, 0);
  const line = state.doc.line(1);
  const range = foldable(state, line.from, line.to);
  assert.equal(doc.slice(range.from, range.to), "\n\\item a\n");

  const group = "\\newcommand{\\x}{\n  body\n}\n";
  const groupState = stateFor(group, 0);
  const groupLine = groupState.doc.line(1);
  const groupRange = foldable(groupState, groupLine.from, groupLine.to);
  assert.ok(groupRange);
  assert.equal(group.slice(groupRange.from, groupRange.to).trim(), "body");
});

test("sections still fold through the line scanner", () => {
  const doc = "\\section{A}\ntext\nmore\n\\section{B}\nend";
  const state = stateFor(doc, 0);
  const line = state.doc.line(1);
  const range = foldable(state, line.from, line.to);
  assert.equal(doc.slice(range.from, range.to), "\ntext\nmore");
});

test("unbalanced input parses and later lines stay highlighted sensibly", () => {
  const cases = {
    "an environment with no end": "\\begin{itemize}\n\\item a\n\\section{Next}\nmore",
    "an unclosed brace": "\\textbf{abc\n\\section{Next}\nmore",
    "an unclosed optional argument": "\\item[abc\n\\section{Next}\nmore",
    "a stray closing brace": "a } b\n\\section{Next}\nmore",
    "a stray end": "\\end{itemize}\n\\section{Next}\nmore",
    "a stray closing bracket": "a ] b \\) c \\]\n\\section{Next}\nmore",
    "a lone backslash": "\\section{Next}\nmore \\",
  };
  for (const [label, doc] of Object.entries(cases)) {
    let tree;
    assert.doesNotThrow(() => { tree = parse(doc); }, label);
    assert.equal(tree.length, doc.length, label);
    const sections = nodes(tree, "SectionName");
    assert.equal(sections.length, 1, `${label}: the section is still a command`);
    assert.equal(doc.slice(sections[0].from, sections[0].to), "\\section");
  }
});

test("an unclosed inline math ends at the blank line", () => {
  const doc = "Let $x = 1\n\n\\section{Next}\nmore";
  const tree = parse(doc);
  assert.deepEqual(errors(tree), []);
  const section = tree.resolveInner(doc.indexOf("\\section") + 1, 1);
  assert.equal(section.parent.name, "Command");
  assert.equal(section.parent.parent.name, "Document");
  const state = stateFor(doc);
  assert.ok(inMath(state, doc.indexOf("x = 1")));
  assert.ok(!inMath(state, doc.indexOf("more")));
});

test("an unclosed display math and an unclosed dollar at the end do not throw", () => {
  for (const doc of ["$$ a + b", "\\[ a", "$", "\\(", "\\begin{align}\na &= b"]) {
    assert.doesNotThrow(() => parse(doc), doc);
  }
  for (const doc of ["$$ a + b", "\\[ a"]) assert.ok(inMath(stateFor(doc), doc.length), doc);
});

// ---- Closing \begin{name} with a matching \end{name} ------------------------

function typeBrace(doc, cursor, extra = {}) {
  const state = stateFor(doc, cursor);
  return state.update({
    changes: { from: cursor, insert: "}" },
    selection: { anchor: cursor + 1 },
    userEvent: "input.type",
    ...extra,
  });
}

test("typing the } of \\begin{name} inserts the matching \\end in the same transaction", () => {
  const tr = typeBrace("\\begin{itemize", 14);
  assert.equal(tr.newDoc.toString(), "\\begin{itemize}\n\\end{itemize}");
  assert.equal(tr.newSelection.main.head, 15);
  assert.ok(tr.isUserEvent("input.type"));
});

test("the inserted \\end keeps the indentation of the \\begin line", () => {
  const tr = typeBrace("  \\begin{center", 15);
  assert.equal(tr.newDoc.toString(), "  \\begin{center}\n  \\end{center}");
});

test("a } stepped over by closeBrackets counts as typing", () => {
  const state = stateFor("\\begin{itemize}", 14);
  const tr = state.update({ selection: { anchor: 15 }, userEvent: "input.type" });
  assert.equal(tr.newDoc.toString(), "\\begin{itemize}\n\\end{itemize}");
  assert.equal(tr.newSelection.main.head, 15);
});

test("no \\end is added when one already follows", () => {
  const doc = "\\begin{itemize\n\\item a\n\\end{itemize}";
  assert.equal(typeBrace(doc, 14).newDoc.toString(), doc.slice(0, 14) + "}" + doc.slice(14));
});

test("a nested environment of the same name still gets its own \\end", () => {
  const doc = "\\begin{itemize}\n\\begin{itemize\n\\end{itemize}";
  const cursor = doc.indexOf("itemize\n\\end") + 7;
  const result = typeBrace(doc, cursor).newDoc.toString();
  assert.equal(result.split("\\end{itemize}").length, 3);
});

test("no \\end is added inside a comment or verbatim text", () => {
  const comment = "% \\begin{itemize";
  assert.equal(typeBrace(comment, comment.length).newDoc.toString(), comment + "}");
  const verbatim = "\\begin{verbatim}\n\\begin{itemize\n\\end{verbatim}";
  const cursor = verbatim.indexOf("itemize") + 7;
  assert.equal(typeBrace(verbatim, cursor).newDoc.toString(), verbatim.slice(0, cursor) + "}" + verbatim.slice(cursor));
});

test("no \\end is added for programmatic or remote changes, or mid-line", () => {
  assert.equal(typeBrace("\\begin{itemize", 14, { userEvent: undefined }).newDoc.toString(), "\\begin{itemize}");
  assert.equal(typeBrace("\\begin{itemize", 14, { userEvent: "input.paste" }).newDoc.toString(), "\\begin{itemize}");
  assert.equal(typeBrace("\\begin{itemize more", 14).newDoc.toString(), "\\begin{itemize} more");
  assert.equal(typeBrace("\\section{intro", 14).newDoc.toString(), "\\section{intro}");
});

test("the extension is exported on its own", () => {
  const state = EditorState.create({ doc: "\\begin{a", selection: { anchor: 8 }, extensions: [autoCloseEnvironment] });
  const tr = state.update({ changes: { from: 8, insert: "}" }, selection: { anchor: 9 }, userEvent: "input.type" });
  assert.equal(tr.newDoc.toString(), "\\begin{a}\n\\end{a}");
});

// ---- Speed ------------------------------------------------------------------

test("a 350 KB document parses in well under 1.5 seconds", () => {
  let text = "";
  while (text.length < 350_000) text += ARTICLE;
  const start = performance.now();
  const tree = latexLanguage.parser.parse(text);
  const elapsed = performance.now() - start;
  assert.equal(tree.length, text.length);
  assert.ok(elapsed < 1500, `parse took ${Math.round(elapsed)} ms`);
});
