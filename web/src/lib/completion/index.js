// Registration point for editor completion. Sources are added through
// languageData, so every language (including plain text) gets them, and each
// source gates itself on the project's format.
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { completionStatus, startCompletion } from "@codemirror/autocomplete";
import { latexCompletion, latexTrigger } from "./latex.js";
import { markdownCompletion, markdownTrigger } from "./markdown.js";
import { orgCompletion, orgTrigger } from "./org.js";
import { quartoCompletion, quartoTrigger } from "./quarto.js";
import { taskCompletion, taskTrigger } from "./tasks.js";
import { typstCompletion, typstTrigger } from "./typst.js";
import { bibliographyTrigger } from "../bibliography.js";
import { sourceFormatOf } from "../formats.js";

/**
 * Format of a project file from its extension. Names match the
 * format values used across the editor.
 * @param {string} path
 * @returns {"latex"|"typst"|"markdown"|"quarto"|"org"|"html"|""}
 */
export function formatOfPath(path) {
  return sourceFormatOf(path);
}

/**
 * Register completion sources for the editor.
 * @param {{ project: () => import("./paths.js").CompletionProject, sources?: Array<(context: import("@codemirror/autocomplete").CompletionContext) => any> }} options
 */
export function completionExtension({ project, sources = [] }) {
  const all = [
    ...sources,
    markdownCompletion(project),
    orgCompletion(project),
    quartoCompletion(project),
    latexCompletion(project),
    typstCompletion(project),
    taskCompletion(project),
  ];
  const entries = all.map((source) => ({ autocomplete: source }));
  return EditorState.languageData.of(() => entries);
}

// Where each format's sources can answer, as predicates over the line's text
// before the cursor (and the state and position, for the few that read further).
// Each predicate lives beside its source and is a superset of where that source
// can return a result. A new source needs its predicate here, or it is never
// woken by typing; completion-triggers.mjs checks the pairs.
const TRIGGERS = {
  latex: [latexTrigger, bibliographyTrigger("latex")],
  markdown: [markdownTrigger, taskTrigger, bibliographyTrigger("markdown")],
  quarto: [markdownTrigger, quartoTrigger, taskTrigger, bibliographyTrigger("quarto")],
  org: [orgTrigger],
  typst: [typstTrigger, bibliographyTrigger("typst")],
};

/**
 * True when some source of `format` can answer at `pos`.
 * @param {string} format
 * @param {import("@codemirror/state").EditorState} state
 * @param {number} pos
 */
export function completionWakes(format, state, pos) {
  const triggers = TRIGGERS[format];
  if (!triggers) return false;
  const line = state.doc.lineAt(pos);
  const before = line.text.slice(0, pos - line.from);
  return triggers.some((trigger) => trigger(before, state, pos));
}

/**
 * Starts completion after a typed character that lands where a source of the
 * file's format can answer. Used with autocompletion({ activateOnTyping: false }),
 * which would otherwise query every source after every keystroke, and run a
 * second editor update for the (usually empty) answer. While a list is open,
 * typing still filters it, as that does not depend on activateOnTyping. The
 * start runs in a microtask because it dispatches, which an update listener may
 * not do. It is an explicit start: CodeMirror queries after 50 ms, not the
 * typing delay.
 * @param {() => { format: string }} project
 */
export function completionWake(project) {
  return EditorView.updateListener.of((update) => {
    if (!update.docChanged || !update.transactions.some((tr) => tr.isUserEvent("input.type"))) return;
    const { state, view } = update;
    if (completionStatus(state) !== null) return;
    const range = state.selection.main;
    if (!range.empty || !completionWakes(project().format, state, range.head)) return;
    queueMicrotask(() => {
      // Anything dispatched in between moves the cursor or the text; the next keystroke asks again.
      if (view.state === state) startCompletion(view);
    });
  });
}
