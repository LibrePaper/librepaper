// LaTeX completion over a real EditorState and CompletionContext, with the
// project accessor faked. Covers label scanning, comment handling, each
// trigger, the \begin insertion, the arity snippet, the package list, and the
// shape of the vendored LaTeX-Workshop list.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { latexCompletion, latexLabels } from "../../src/lib/completion/latex.js";
import { PACKAGES } from "../../src/lib/completion/latex-packages.js";

// The edited document is the text up to the cursor, which is the end of it.
function complete(doc, { format = "latex", texts = {}, files = [], path = "main.tex", pos = doc.length } = {}) {
  const state = EditorState.create({ doc });
  const context = new CompletionContext(state, pos, true);
  return latexCompletion(() => ({ path, format, texts, files }))(context);
}

const labelsOf = (result) => result.options.map((option) => option.label).sort();

// Runs a completion's apply on a document and returns the new text.
function applyTo(option, doc, from, to) {
  let out = null;
  const state = EditorState.create({ doc });
  option.apply({ state, dispatch: (tr) => { out = tr.state.doc.toString(); } }, option, from, to);
  return out;
}

// --- latexLabels -----------------------------------------------------------
{
  const labels = latexLabels({
    "main.tex": "\\label{sec:intro}\n% \\label{hidden}\n\\label{fig:one} \\label{plain}\n\\label{eq:a} 100\\% \\label{shown}",
    "chap/two.ltx": "\\section{Two}\\label{sec:two}",
    "notes.md": "\\label{nope}",
  });
  assert.deepEqual(labels, [
    { id: "sec:two", kind: "sec", path: "chap/two.ltx", line: 1 },
    { id: "sec:intro", kind: "sec", path: "main.tex", line: 1 },
    { id: "fig:one", kind: "fig", path: "main.tex", line: 3 },
    { id: "plain", kind: "label", path: "main.tex", line: 3 },
    { id: "eq:a", kind: "eq", path: "main.tex", line: 4 },
    { id: "shown", kind: "label", path: "main.tex", line: 4 },
  ]);
}

// --- \ref and friends -------------------------------------------------------
{
  const texts = { "main.tex": "\\label{fig:b}", "a.tex": "\\label{sec:a}" };
  const doc = "\\label{fig:b}\n\\ref{";
  const result = complete(doc, { texts });
  assert.ok(result);
  assert.equal(result.from, doc.length);
  assert.deepEqual(labelsOf(result), ["fig:b", "sec:a"]);
  assert.equal(result.options.find((option) => option.label === "fig:b").detail, "main.tex:1");
}

// Comma lists: the token is what follows the last comma and its spaces.
{
  const doc = "\\cref{sec:a, fi";
  const result = complete(doc, { texts: { "a.tex": "\\label{sec:a}\n\\label{fig:x}" } });
  assert.ok(result);
  assert.equal(result.from, doc.length - 2);
  assert.ok(labelsOf(result).includes("fig:x"));
}

// --- \input, \includegraphics, \bibliography, \usepackage ------------------
{
  const files = ["main.tex", "chap/one.tex", "figs/a.png", "notes.md", "refs.bib", "other.bib"];
  const input = complete("\\input{", { files });
  assert.deepEqual(labelsOf(input), ["chap/one"]);

  const graphic = complete("\\includegraphics[width=3cm]{", { files });
  assert.deepEqual(labelsOf(graphic), ["figs/a.png"]);

  const bibliography = complete("\\bibliography{", { files });
  assert.deepEqual(labelsOf(bibliography), ["other", "refs"]);

  const addbib = complete("\\addbibresource{", { files });
  assert.deepEqual(labelsOf(addbib), ["other.bib", "refs.bib"]);

  const packages = complete("\\usepackage[utf8]{");
  assert.ok(labelsOf(packages).includes("amsmath"));
  assert.ok(labelsOf(packages).includes("graphicx"));
}

// --- \begin{ insertion ------------------------------------------------------
{
  const doc = "\\begin{ite";
  const result = complete(doc, { texts: { "main.tex": "\\newenvironment{proof}{}{}" } });
  const itemize = result.options.find((option) => option.label === "itemize");
  assert.ok(itemize);
  const inserted = applyTo(itemize, doc, 7, doc.length);
  assert.ok(inserted.startsWith("\\begin{itemize}\n"));
  assert.ok(inserted.includes("\\item"));
  assert.ok(inserted.endsWith("\n\\end{itemize}"));

  // An \end on the next line is not repeated.
  const open = "\\begin{ite\n\\end{itemize}";
  const closedResult = complete(open, { pos: 10 });
  const closedOption = closedResult.options.find((option) => option.label === "itemize");
  const once = applyTo(closedOption, open, 7, 10);
  assert.equal(once.split("\\end{itemize}").length - 1, 1);
}

// Environments defined in the project and from packages loaded in the project.
{
  const texts = { "main.tex": "\\usepackage{mathtools}\n\\newenvironment{proof}{}{}" };
  const result = complete("\\begin{", { texts });
  const labels = labelsOf(result);
  assert.ok(labels.includes("proof"));
  assert.ok(labels.includes("dcases"));
  assert.ok(labels.includes("itemize"));
}

// --- \partial commands ----------------------------------------------------
{
  const doc = "\\cr";
  const result = complete(doc, { texts: { "main.tex": "\\usepackage{cleveref}" } });
  assert.equal(result.from, 1);
  assert.ok(labelsOf(result).includes("cref"));
}

// A project \newcommand with arity 2 completes with two argument groups, and
// the name is part of the snippet, so it is inserted after the backslash.
{
  const doc = "\\pa";
  const result = complete(doc, { texts: { "main.tex": "\\newcommand{\\pair}[2]{(#1, #2)}" } });
  const pair = result.options.find((option) => option.label === "pair");
  assert.ok(pair);
  assert.equal(result.from, 1);
  assert.equal(applyTo(pair, doc, 1, doc.length), "\\pair{}{}");
}

// Project definitions come first and win over the built-in list.
{
  const result = complete("\\", { texts: { "main.tex": "\\def\\begin{}\n\\newcommand\\hello{hi}" } });
  const names = result.options.map((option) => option.label);
  assert.ok(names.includes("hello"));
  assert.equal(names.filter((name) => name === "begin").length, 1);
}

// --- Null cases -------------------------------------------------------------
{
  assert.equal(complete("\\ref{", { format: "markdown" }), null);
  assert.equal(complete("\\input{", { format: "typst" }), null);
  assert.equal(complete("% see \\ref{"), null);
  assert.equal(complete("\\section{x} % \\input{"), null);
  // An escaped % is not a comment, so the trigger still completes.
  assert.ok(complete("\\section{x} 100\\% \\input{", { files: ["a.tex"] }));
}

// --- Package lists ----------------------------------------------------------
// Every package snippet starts with its own command name, so the name is
// never lost when the snippet replaces the typed text.
{
  for (const [pkg, entry] of Object.entries(PACKAGES)) {
    for (const command of entry.commands) {
      if (command.snippet !== undefined) {
        assert.ok(command.snippet.startsWith(command.name), `${pkg}: ${command.name}`);
      }
    }
  }
}

// --- latex-workshop.json ----------------------------------------------------
{
  const data = JSON.parse(readFileSync(new URL("../../src/lib/completion/latex-workshop.json", import.meta.url), "utf8"));
  assert.ok(data.commands.length > 0);
  assert.ok(data.environments.length > 0);
  for (const item of [...data.commands, ...data.environments]) {
    const snippet = item.snippet ?? "";
    assert.equal(snippet.includes("$0"), false, `${item.name}: ${snippet}`);
    assert.equal(snippet.includes("TM_"), false, `${item.name}: ${snippet}`);
  }
}
