// LaTeX completion over a real EditorState and CompletionContext, with the
// project accessor faked. Covers label lookup through the document index,
// comment handling, each trigger, the \begin insertion, the arity snippet, the
// package list, and the shape of the vendored LaTeX-Workshop list.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { latexCompletion } from "../../src/lib/completion/latex.js";
import { PACKAGES } from "../../src/lib/completion/latex-packages.js";
import { indexTexts, labelList } from "../../src/lib/document-index/labels.js";

// The edited document is the text up to the cursor, which is the end of it.
// `labels` stands for the editor's live index, when given.
function complete(doc, { format = "latex", texts = {}, files = [], path = "main.tex", pos = doc.length, labels } = {}) {
  const state = EditorState.create({ doc });
  const context = new CompletionContext(state, pos, true);
  return latexCompletion(() => ({ path, format, texts, files, ...(labels ? { labels } : {}) }))(context);
}

const labelsOf = (result) => result.options.map((option) => option.label).sort();

// Runs a completion's apply on a document and returns the new text.
function applyTo(option, doc, from, to) {
  let out = null;
  const state = EditorState.create({ doc });
  option.apply({ state, dispatch: (tr) => { out = tr.state.doc.toString(); } }, option, from, to);
  return out;
}

// --- labels from the document index ----------------------------------------
{
  // Comments are ignored, a .md file is not LaTeX, and paths sort before
  // offsets: chap/two.ltx comes before main.tex.
  const texts = {
    "main.tex": "\\label{sec:intro}\n% \\label{hidden}\n\\label{fig:one} \\label{plain}\n\\label{eq:a} 100\\% \\label{shown}",
    "chap/two.ltx": "\\section{Two}\\label{sec:two}",
    "notes.md": "\\label{nope}",
  };
  const labels = labelList(indexTexts(texts), texts, { formats: new Set(["latex"]) })
    .map(({ id, path, line }) => ({ id, path, line }));
  assert.deepEqual(labels, [
    { id: "sec:two", path: "chap/two.ltx", line: 1 },
    { id: "sec:intro", path: "main.tex", line: 1 },
    { id: "fig:one", path: "main.tex", line: 3 },
    { id: "plain", path: "main.tex", line: 3 },
    { id: "eq:a", path: "main.tex", line: 4 },
    { id: "shown", path: "main.tex", line: 4 },
  ]);
}

// Without the editor's index, completion indexes the texts itself, and the
// edited document is read from the editor, not from `texts`.
{
  const doc = "\\label{fig:b}\n\\ref{";
  const result = complete(doc, { texts: { "main.tex": "\\label{stale}", "a.tex": "\\label{sec:a}" } });
  assert.deepEqual(labelsOf(result), ["fig:b", "sec:a"]);
}

// With the editor's index, completion asks it for the labels, first file and
// LaTeX only, and shows what it answers, with the title as `info`.
{
  const asked = [];
  const labels = (options) => {
    asked.push(options);
    return [{ id: "fig:live", path: "main.tex", line: 7, title: "A plot" }, { id: "sec:x", path: "a.tex", line: 2 }];
  };
  const result = complete("\\ref{", { labels, texts: { "main.tex": "" } });
  assert.deepEqual(asked, [{ first: "main.tex", formats: new Set(["latex"]) }]);
  assert.deepEqual(result.options, [
    { label: "fig:live", type: "variable", detail: "main.tex:7", info: "A plot" },
    { label: "sec:x", type: "variable", detail: "a.tex:2" },
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
  const result = complete(doc, { texts: { "preamble.tex": "\\newenvironment{proof}{}{}" } });
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

// closeBrackets() leaves a "}" after the cursor. Accepting replaces it, so
// the brace is not doubled, for a partial name and for a whole one.
{
  // snippet() indents the body with the state's indent unit, two spaces here.
  const expected = "\\begin{itemize}\n  \\item \n\\end{itemize}";
  const partial = complete("\\begin{ite}", { pos: 10 }).options.find((option) => option.label === "itemize");
  assert.equal(applyTo(partial, "\\begin{ite}", 7, 10), expected);

  const whole = complete("\\begin{itemize}", { pos: 14 }).options.find((option) => option.label === "itemize");
  assert.equal(applyTo(whole, "\\begin{itemize}", 7, 14), expected);

  // Without a brace after the cursor, nothing else is removed.
  const bare = complete("\\begin{ite", { pos: 10 }).options.find((option) => option.label === "itemize");
  assert.equal(applyTo(bare, "\\begin{ite", 7, 10), expected);
}

// Environments defined in the project and from packages loaded in the project.
{
  const texts = { "preamble.tex": "\\usepackage{mathtools}\n\\newenvironment{proof}{}{}" };
  const result = complete("\\begin{", { texts });
  const labels = labelsOf(result);
  assert.ok(labels.includes("proof"));
  assert.ok(labels.includes("dcases"));
  assert.ok(labels.includes("itemize"));
}

// --- \partial commands ----------------------------------------------------
{
  const doc = "\\cr";
  const result = complete(doc, { texts: { "preamble.tex": "\\usepackage{cleveref}" } });
  assert.equal(result.from, 1);
  assert.ok(labelsOf(result).includes("cref"));
}

// A project \newcommand with arity 2 completes with two argument groups, and
// the name is part of the snippet, so it is inserted after the backslash.
{
  const doc = "\\pa";
  const result = complete(doc, { texts: { "preamble.tex": "\\newcommand{\\pair}[2]{(#1, #2)}" } });
  const pair = result.options.find((option) => option.label === "pair");
  assert.ok(pair);
  assert.equal(result.from, 1);
  assert.equal(applyTo(pair, doc, 1, doc.length), "\\pair{}{}");
}

// Project definitions come first and win over the built-in list.
{
  const result = complete("\\", { texts: { "preamble.tex": "\\def\\begin{}\n\\newcommand\\hello{hi}" } });
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

// The subfigure placement is optional, the width is required.
{
  const subfigure = PACKAGES.subcaption.environments.find((item) => item.name === "subfigure");
  assert.equal(subfigure.snippet, "[t]{${1:0.45}\\linewidth}\n\t${0}");
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

// A command whose first argument has a default leaves the optional one out.
{
  const doc = "\\fo";
  const result = complete(doc, { texts: { "preamble.tex": "\\newcommand{\\foo}[2][default]{#1 #2}" } });
  const foo = result.options.find((option) => option.label === "foo");
  assert.equal(applyTo(foo, doc, 1, doc.length), "\\foo{}");
}
