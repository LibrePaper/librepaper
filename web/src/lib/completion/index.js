// Registration point for editor completion. Sources are added through
// languageData, so every language (including plain text) gets them, and each
// source gates itself on the project's format.
import { EditorState } from "@codemirror/state";
import { markdownCompletion, markdownLabels } from "./markdown.js";

/**
 * Format of a project file from its extension. Names match the
 * format values used across the editor.
 * @param {string} path
 * @returns {"latex"|"typst"|"markdown"|"quarto"|"html"|""}
 */
export function formatOfPath(path) {
  const lower = String(path || "").toLowerCase();
  if (/\.(?:tex|ltx|sty|cls)$/.test(lower)) return "latex";
  if (lower.endsWith(".typ")) return "typst";
  if (lower.endsWith(".qmd")) return "quarto";
  if (/\.(?:md|markdown)$/.test(lower)) return "markdown";
  if (/\.(?:html|htm)$/.test(lower)) return "html";
  return "";
}

/**
 * Register completion sources for the editor.
 * @param {{ project: () => import("./paths.js").CompletionProject, sources?: Array<(context: import("@codemirror/autocomplete").CompletionContext) => any> }} options
 */
export function completionExtension({ project, sources = [] }) {
  const all = [...sources, markdownCompletion(project)];
  const entries = all.map((source) => ({ autocomplete: source }));
  return EditorState.languageData.of(() => entries);
}

/** Labels for cross references, dispatched by format. */
export function projectLabels(texts, format) {
  if (format === "markdown" || format === "quarto") return markdownLabels(texts);
  return [];
}
