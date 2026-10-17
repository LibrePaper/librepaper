// Completion for Quarto (.qmd) documents: front matter keys and values, code
// cell options, and cross-references to labels in the project.
//
// The option lists come from quarto-schema.json (see NOTICE-quarto.md). They
// are built once at load. Cross-references read other project files only
// after the trigger matches, so a keystroke elsewhere costs nothing.
import schema from "./quarto-schema.json" with { type: "json" };

/** @typedef {{ id: string, kind: string, path: string, line: number }} QuartoLabel */

const KINDS = ["fig", "tbl", "sec", "eq", "lst", "thm", "lem", "cor", "prp", "cnj", "def", "exm", "exr"];
const KIND_ALTERNATION = KINDS.join("|");
const LABEL = new RegExp(`^(?:${KIND_ALTERNATION})-[\\w-]+$`);
const LABEL_PARTIAL = new RegExp(`^(?:${KIND_ALTERNATION})-[\\w-]*$`);
const ATTRIBUTE_ID = new RegExp(`\\{#((?:${KIND_ALTERNATION})-[\\w-]+)(?=[\\s}])`, "g");
const CELL_LABEL = /^#\|\s*label:\s*["']?([^\s"']+)/;
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

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

/**
 * Fence state for every line: the open fence while the line sits inside a
 * code block (the opening and closing lines included), else null.
 * @param {string[]} lines
 * @returns {Array<{ marker: string, length: number, info: string } | null>}
 */
function fenceStates(lines) {
  const states = new Array(lines.length).fill(null);
  let fence = null;
  lines.forEach((line, index) => {
    const match = line.match(FENCE);
    if (!fence) {
      // A backtick fence cannot have a backtick in its info string.
      if (match && !(match[1][0] === "`" && match[2].includes("`"))) {
        fence = { marker: match[1][0], length: match[1].length, info: match[2].trim() };
        states[index] = fence;
      }
      return;
    }
    states[index] = fence;
    if (match && match[1][0] === fence.marker && match[1].length >= fence.length && !match[2].trim()) fence = null;
  });
  return states;
}

/** True when the line sits between the leading `---` lines of the document. */
function inFrontMatter(lines, index) {
  if (index === 0 || !/^---\s*$/.test(lines[0] || "")) return false;
  for (let i = 1; i < lines.length; i++) {
    if (/^(?:---|\.\.\.)\s*$/.test(lines[i])) return index < i;
  }
  return false;
}

/**
 * Labels in one Quarto source: cell labels inside executable cells and
 * attribute ids outside code.
 * @param {string} text
 * @param {string} path
 * @returns {QuartoLabel[]}
 */
function labelsIn(text, path) {
  const lines = text.split(/\r?\n/);
  const states = fenceStates(lines);
  const found = [];
  const add = (id, index) => found.push({ id, kind: id.slice(0, id.indexOf("-")), path, line: index + 1 });
  lines.forEach((line, index) => {
    const fence = states[index];
    if (fence) {
      const match = fence.info.startsWith("{") && line.match(CELL_LABEL);
      if (match && LABEL.test(match[1])) add(match[1], index);
      return;
    }
    for (const match of line.replace(/`[^`]*`/g, "").matchAll(ATTRIBUTE_ID)) add(match[1], index);
  });
  return found;
}

/**
 * Cross-reference targets in the project's Quarto files.
 * @param {Record<string, string>} texts
 * @returns {QuartoLabel[]}
 */
export function quartoLabels(texts) {
  const found = [];
  for (const [path, text] of Object.entries(texts || {})) {
    if (!/\.qmd$/i.test(path)) continue;
    found.push(...labelsIn(text, path));
  }
  return found;
}

/** Cross-reference options: the edited document first, then other files, one per id. */
function referenceOptions(project, documentText, documentPath) {
  const seen = new Set();
  const options = [];
  const all = [...labelsIn(documentText, documentPath), ...quartoLabels(project.texts).filter((label) => label.path !== documentPath)];
  for (const label of all) {
    if (seen.has(label.id)) continue;
    seen.add(label.id);
    options.push({ label: label.id, type: "variable", detail: `${label.path}:${label.line}` });
  }
  return options;
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
