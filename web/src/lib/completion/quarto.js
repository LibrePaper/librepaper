// Completion for Quarto (.qmd) documents: front matter keys and values, code
// cell options, and cross-references to labels in the project.
//
// The option lists come from quarto-schema.json (see NOTICE-quarto.md). They
// are built once at load. Cross-references read other project files only
// after the trigger matches, so a keystroke elsewhere costs nothing.
import schema from "./quarto-schema.json" with { type: "json" };
import { indexTexts, labelList } from "../document-index/labels.js";
import { KINDS, LABEL_PARTIAL, fenceStates } from "../quarto-labels.js";

/** @typedef {{ path: string, texts: Record<string, string>, labels?: (options: { first?: string, formats?: Set<string> }) => Array<{ id: string, path: string, line: number, title?: string }> }} ReferenceProject */

export { quartoLabels } from "../quarto-labels.js";

const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
const option = (entry, type) => ({
  label: entry.name,
  apply: `${entry.name}: `,
  type,
  ...(entry.description ? { info: entry.description } : {}),
});
const valueOptions = (values) => values.map((label) => ({ label, type: "constant" }));

// Built once. Hidden entries are already gone from the JSON.
const DOCUMENT_KEYS = schema.document.map((entry) => option(entry, "property")).sort(byName);
const CELL_KEYS = schema.cell.map((entry) => option(entry, "property")).sort(byName);
const DOCUMENT_VALUES = new Map(schema.document.filter((entry) => entry.values.length).map((entry) => [entry.name, valueOptions(entry.values)]));
const CELL_VALUES = new Map(schema.cell.filter((entry) => entry.values.length).map((entry) => [entry.name, valueOptions(entry.values)]));

/** True when the line sits between the leading `---` lines of the document. */
function inFrontMatter(lines, index) {
  if (index === 0 || !/^---\s*$/.test(lines[0] || "")) return false;
  for (let i = 1; i < lines.length; i++) {
    if (/^(?:---|\.\.\.)\s*$/.test(lines[i])) return index < i;
  }
  return false;
}

/**
 * Cross-reference options: the edited document first, then other files, one per id.
 * @param {ReferenceProject} project
 * @param {string} documentText the live text of the edited document
 * @param {string} documentPath
 * @returns {Array<{ label: string, type: string, detail: string, info?: string }>}
 */
function referenceOptions(project, documentText, documentPath) {
  const formats = new Set(["quarto"]);
  const texts = { ...project.texts, [documentPath]: documentText };
  // The editor's live index already holds the edited document: it refreshes
  // before answering.
  const labels = project.labels
    ? project.labels({ first: documentPath, formats })
    : labelList(indexTexts(texts), texts, { first: documentPath, formats });
  return labels.map((label) => ({
    label: label.id,
    type: "variable",
    detail: `${label.path}:${label.line}`,
    ...(label.title ? { info: label.title } : {}),
  }));
}

/**
 * @param {() => { format: string, path: string, texts: Record<string, string> }} project
 * @returns {(context: import("@codemirror/autocomplete").CompletionContext) => import("@codemirror/autocomplete").CompletionResult | null}
 */
export function quartoCompletion(project) {
  return (context) => {
    const current = project();
    if (current.format !== "quarto") return null;

    const doc = context.state.doc;
    const text = doc.toString();
    const line = doc.lineAt(context.pos);
    const index = line.number - 1;
    const linePrefix = line.text.slice(0, context.pos - line.from);
    const lines = text.split(/\r?\n/);

    if (inFrontMatter(lines, index)) {
      const value = linePrefix.match(/^([A-Za-z][\w-]*):[ \t]+(\S*)$/);
      if (value) {
        const options = DOCUMENT_VALUES.get(value[1]);
        if (!options) return null;
        return { from: context.pos - value[2].length, options, validFor: /^\S*$/ };
      }
      const key = linePrefix.match(/^[A-Za-z0-9_-]*$/);
      if (key) return { from: context.pos - key[0].length, options: DOCUMENT_KEYS, validFor: /^[\w-]*$/ };
      return null;
    }

    const states = fenceStates(lines);
    const fence = states[index];
    if (fence) {
      if (!fence.info.startsWith("{") || !linePrefix.startsWith("#| ")) return null;
      const cellValue = linePrefix.match(/^#\| ([\w-]+):[ \t]*(\S*)$/);
      if (cellValue) {
        const options = CELL_VALUES.get(cellValue[1]);
        if (!options) return null;
        return { from: context.pos - cellValue[2].length, options, validFor: /^\S*$/ };
      }
      const cellKey = linePrefix.match(/^#\| ([\w-]*)$/);
      if (cellKey) return { from: context.pos - cellKey[1].length, options: CELL_KEYS, validFor: /^[\w-]*$/ };
      return null;
    }

    // Cross-references stay out of inline code, where an odd backtick is still open.
    if ((linePrefix.match(/`/g) || []).length % 2) return null;
    const reference = linePrefix.match(/(?:^|[^\w@])@([\w-]*)$/);
    if (!reference) return null;
    const partial = reference[1];
    const typed = LABEL_PARTIAL.test(partial) || (partial && !partial.includes("-") && KINDS.some((kind) => kind.startsWith(partial)));
    if (!typed && !(partial === "" && context.explicit)) return null;

    const options = referenceOptions(current, text, current.path);
    if (!options.length) return null;
    return { from: context.pos - partial.length, options, validFor: /^[\w-]*$/ };
  };
}
