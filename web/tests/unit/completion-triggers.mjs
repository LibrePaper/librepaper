// Completion triggers: typing wakes completion only where a source of the file's
// format can answer. Each source has a predicate beside it, and the editor starts
// completion only when one is true (completion/index.js). The positive cases here
// are the cursor positions the sources' own tests use to get results; if a source
// is extended to answer somewhere new and its predicate is not, this file fails.
// The negative cases are prose, where the gate must stay shut.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { completionWakes } from "../../src/lib/completion/index.js";
import { latexCompletion, latexTrigger } from "../../src/lib/completion/latex.js";
import { markdownTrigger } from "../../src/lib/completion/markdown.js";
import { orgTrigger } from "../../src/lib/completion/org.js";
import { quartoCompletion, quartoTrigger } from "../../src/lib/completion/quarto.js";
import { taskCompletion, taskTrigger } from "../../src/lib/completion/tasks.js";
import { typstTrigger } from "../../src/lib/completion/typst.js";
import { bibliographyTrigger } from "../../src/lib/bibliography.js";

// The last "|" marks the cursor (a Quarto option line starts with one).
function at(doc) {
  const pos = doc.lastIndexOf("|");
  const text = doc.slice(0, pos) + doc.slice(pos + 1);
  return { state: EditorState.create({ doc: text }), pos };
}
const wakes = (format, doc) => {
  const { state, pos } = at(doc);
  return completionWakes(format, state, pos);
};

const positives = {
  latex: [
    "\\|",
    "\\begin{ite|",
    "\\begin{|",
    "\\ref{|",
    "\\includegraphics[width=3cm]{|",
    "\\input{|",
    "\\bibliography{|",
    "\\addbibresource{|",
    "\\usepackage[utf8]{|",
    "\\section{x} 100\\% \\input{|",
    "\\cite{smi|",
    // A \cite argument wrapped onto a line with no backslash of its own.
    "\\cite{a,\n  smi|",
    "\\parencite[p. 3]{a,\n  b|",
  ],
  markdown: [
    "![a](fi|",
    "![a](<fig|",
    "[x](|",
    "# Overview\n\nSee [here](|",
    "[x](other.md#|",
    "See [a][|\n\n[one]: http://x",
    "Text[^|\n\n[^note]: body",
    "See @fi|",
    "mail me at jo@fi|",
    "- [ ] pri|",
    "1. [ ] pri|",
    "- [ ] d|",
    "- [ ] c #|",
    "- [ ] c #w|",
    "- [ ] c assignee:|",
    "- [ ] c assignee:alice,c|",
    "- [ ] due:|",
    "- [ ] priority:  2|",
    "- [ ] write due:2026-01-02 pri|",
  ],
  quarto: [
    "---\nformat: ht|\n---\n",
    "---\nti|\n---\n",
    "---\ntitle: |\n---\n",
    "```{r}\n#| ec|\n```\n",
    "```{python}\n#| echo: |\n```\n",
    "```{r}\n#| echo: fa|\n```\n",
    "Intro {#sec-a}\n\nSee @fi|",
    "See @|",
    "See @fig-|",
    "[x](|",
    "Text[^|",
    "- [ ] pri|",
  ],
  org: [
    "[[file:|",
    "[[./|",
    "[[|\n<<intro>>",
    "#+BEGIN_|",
    "#+TI|",
    "#+BIBLIOGRAPHY: |",
    "#+INCLUDE: \"|",
    "See [cite:@d|",
    "[cite:@d|",
  ],
  typst: [
    "#|",
    "#le|",
    "#let x = f|",
    "#image(\"fig/|",
    "#set text(font: \"Li|",
    "x.fo|",
    "  let y = fo|",
    "See <sec|",
    "A formula $x + y|",
    "Cited by @sm|",
    "See @|",
  ],
};
for (const [format, docs] of Object.entries(positives)) {
  for (const doc of docs) assert.equal(wakes(format, doc), true, `${format} should wake at ${JSON.stringify(doc)}`);
}

// Plain prose wakes nothing.
const negatives = {
  latex: ["Plain prose words|", "A sentence with 100% prose|", "Hello wor|", "\\cite{a}\nplain wo|", "x\n\nprose|"],
  markdown: ["Hello world|", "# Heading words|", "- plain list item|", "- [ ] zz|", "A [link] and text|", "Some text #wr|"],
  quarto: ["Hello world|", "text\n---\nti|\n---\n", "---\ntitle: x\n---\nBody|", "---\ntitle: x\n---\nBody words here|", "```{r}\nx <- 1|\n```\n"],
  org: ["Plain org prose|", "* Heading words|", "A bracket [one] and more|", "- list item|"],
  typst: ["Plain prose words|", "= A heading|", "- list item here|", "Hello, world|", "Hello,|", "Note:|", "#let |", "!|"],
  html: ["<p>word|", "\\ref{|", "See @fi|"],
  "": ["word|"],
};
for (const [format, docs] of Object.entries(negatives)) {
  for (const doc of docs) assert.equal(wakes(format, doc), false, `${format} should not wake at ${JSON.stringify(doc)}`);
}

// Each predicate on its own, so a failure names the source that lost its trigger.
{
  const check = (trigger, doc, expected) => {
    const { state, pos } = at(doc);
    const line = state.doc.lineAt(pos);
    assert.equal(trigger(line.text.slice(0, pos - line.from), state, pos), expected, JSON.stringify(doc));
  };
  check(latexTrigger, "\\ref{|", true);
  check(latexTrigger, "prose|", false);
  check(markdownTrigger, "[a](|", true);
  check(markdownTrigger, "[a][|", true);
  check(markdownTrigger, "[^|", true);
  check(markdownTrigger, "[a] and|", false);
  check(orgTrigger, "[[|", true);
  check(orgTrigger, "#+|", true);
  check(orgTrigger, "@|", true);
  check(orgTrigger, "prose|", false);
  check(taskTrigger, "- [ ] pri|", true);
  check(taskTrigger, "- [ ] zz|", false);
  check(taskTrigger, "- [ ]|", false);
  check(taskTrigger, "- pri|", false);
  check(quartoTrigger, "#| ec|", true);
  check(quartoTrigger, "See @fi|", true);
  check(quartoTrigger, "---\nti|\n---", true);
  check(quartoTrigger, "ti|", false);
  check(typstTrigger, "#|", true);
  check(typstTrigger, "word|", false);
  check(typstTrigger, "#let |", false);
  check(bibliographyTrigger("markdown"), "See @sm|", true);
  check(bibliographyTrigger("markdown"), "See sm|", false);
  check(bibliographyTrigger("latex"), "\\cite{a,\n b|", true);
  check(bibliographyTrigger("latex"), "\\cite{a}\n b|", false);
  // A wrapped argument is read back at most 20 lines, as the source does.
  check(bibliographyTrigger("latex"), `\\cite{a,\n${"x\n".repeat(25)}b|`, false);
}

// The predicates and the sources agree on cursor positions the sources answer.
{
  const project = (format) => () => ({ path: format === "quarto" ? "main.qmd" : format === "markdown" ? "main.md" : "main.tex", format, texts: {}, files: [] });
  const answered = (source, doc, explicit = false) => {
    const { state, pos } = at(doc);
    return source(new CompletionContext(state, pos, explicit));
  };
  assert.ok(answered(latexCompletion(project("latex")), "\\begin{ite|"));
  assert.ok(answered(latexCompletion(project("latex")), "\\|"));
  assert.ok(answered(quartoCompletion(project("quarto")), "---\nformat: ht|\n---\n"));
  assert.ok(answered(quartoCompletion(project("quarto")), "```{r}\n#| ec|\n```\n"));
  assert.ok(answered(taskCompletion(project("markdown")), "- [ ] pri|"));
  for (const [format, doc] of [
    ["latex", "\\begin{ite|"],
    ["latex", "\\|"],
    ["quarto", "---\nformat: ht|\n---\n"],
    ["quarto", "```{r}\n#| ec|\n```\n"],
    ["markdown", "- [ ] pri|"],
  ]) assert.equal(wakes(format, doc), true, `${format} ${JSON.stringify(doc)}`);
}
