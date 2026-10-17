// Core LaTeX commands and environments, offered with no \usepackage. Runs
// against a real EditorState and CompletionContext, with the project faked.
import assert from "node:assert/strict";
import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { latexCompletion } from "../../src/lib/completion/latex.js";
import { CORE } from "../../src/lib/completion/latex-packages.js";

function complete(doc, { texts = {} } = {}) {
  const state = EditorState.create({ doc });
  const context = new CompletionContext(state, doc.length, true);
  return latexCompletion(() => ({ path: "main.tex", format: "latex", texts, files: [] }))(context);
}

// Runs a completion's apply on a document and returns the new text.
function applyTo(option, doc, from, to) {
  let out = null;
  const state = EditorState.create({ doc });
  option.apply({ state, dispatch: (tr) => { out = tr.state.doc.toString(); } }, option, from, to);
  return out;
}

const labelsOf = (result) => result.options.map((option) => option.label);

// Core commands are offered after a backslash with no \usepackage in the project.
{
  const result = complete("\\");
  const labels = labelsOf(result);
  for (const name of ["section", "textbf", "frac", "documentclass"]) {
    assert.ok(labels.includes(name), name);
  }
}

// Core environments are offered after \begin{, tabular included.
{
  const result = complete("\\begin{");
  assert.ok(labelsOf(result).includes("tabular"));
  assert.ok(labelsOf(result).includes("figure"));
}

// The tabular template keeps its column default, and its \end closes it.
{
  const doc = "\\begin{tab";
  const tabular = complete(doc).options.find((option) => option.label === "tabular");
  assert.equal(
    applyTo(tabular, doc, 7, doc.length),
    "\\begin{tabular}{lc}\n  \n\\end{tabular}",
  );
}

// A core command inserts its snippet with the name kept.
{
  const doc = "\\sec";
  const section = complete(doc).options.find((option) => option.label === "section");
  assert.equal(applyTo(section, doc, 1, doc.length), "\\section{}");
}

// Project definitions come first: a project \section wins over the core entry.
{
  const result = complete("\\", { texts: { "preamble.tex": "\\newcommand\\section{x}" } });
  const sections = result.options.filter((option) => option.label === "section");
  assert.equal(sections.length, 1);
  assert.equal(sections[0].detail, "defined in project");
}

// Each core name appears once in the command list, and the environments too.
{
  const names = CORE.commands.map((item) => item.name);
  assert.equal(new Set(names).size, names.length);
  const envs = CORE.environments.map((item) => item.name);
  assert.equal(new Set(envs).size, envs.length);
}

// Every core snippet starts with its own command name, so the name survives.
for (const command of CORE.commands) {
  if (command.snippet !== undefined) {
    assert.ok(command.snippet.startsWith(command.name), command.name);
  }
}
