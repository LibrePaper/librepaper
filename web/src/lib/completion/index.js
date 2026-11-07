// Registration point for editor completion. Sources are added through
// languageData, so every language (including plain text) gets them, and each
// source gates itself on the project's format.
import { EditorState } from "@codemirror/state";
import { latexCompletion } from "./latex.js";
import { markdownCompletion } from "./markdown.js";
import { quartoCompletion } from "./quarto.js";
import { taskCompletion } from "./tasks.js";
import { typstCompletion } from "./typst.js";
import { sourceFormatOf } from "../formats.js";

/**
 * Format of a project file from its extension. Names match the
 * format values used across the editor.
 * @param {string} path
 * @returns {"latex"|"typst"|"markdown"|"quarto"|"html"|""}
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
    quartoCompletion(project),
    latexCompletion(project),
    typstCompletion(project),
    taskCompletion(project),
  ];
  const entries = all.map((source) => ({ autocomplete: source }));
  return EditorState.languageData.of(() => entries);
}
