// Quarto cross-reference labels: the kinds Quarto numbers, the forms that
// declare a label, and the scan that finds them. Shared by Quarto completion
// and the document index's Markdown adapter, and imports nothing, so neither
// depends on the other.

/** @typedef {{ id: string, kind: string, path: string, line: number }} QuartoLabel */

export const KINDS = ["fig", "tbl", "sec", "eq", "lst", "thm", "lem", "cor", "prp", "cnj", "def", "exm", "exr"];
export const KIND_ALTERNATION = KINDS.join("|");
export const LABEL = new RegExp(`^(?:${KIND_ALTERNATION})-[\\w-]+$`);
export const LABEL_PARTIAL = new RegExp(`^(?:${KIND_ALTERNATION})-[\\w-]*$`);
export const ATTRIBUTE_ID = new RegExp(`\\{#((?:${KIND_ALTERNATION})-[\\w-]+)(?=[\\s}])`, "g");
export const CELL_LABEL = /^#\|\s*label:\s*["']?([^\s"']+)/;
export const HEADER_LABEL = /[\s,]label\s*=\s*["']?([^\s"',}]+)/;
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

/**
 * Fence state for every line: the open fence while the line sits inside a
 * code block (the opening and closing lines included), else null.
 * @param {string[]} lines
 * @returns {Array<{ marker: string, length: number, info: string } | null>}
 */
export function fenceStates(lines) {
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
  // One label per executable cell. A "#| label:" line overrides a label in
  // the header ({r, label="fig-x"}), as Quarto reads them.
  const cells = new Map();
  lines.forEach((line, index) => {
    const fence = states[index];
    if (fence) {
      if (!fence.info.startsWith("{")) return;
      const cell = cells.get(fence) || {};
      cells.set(fence, cell);
      if (states[index - 1] !== fence) {
        const header = fence.info.match(HEADER_LABEL);
        if (header && LABEL.test(header[1])) cell.header = { id: header[1], index };
        return;
      }
      const match = line.match(CELL_LABEL);
      if (match && LABEL.test(match[1]) && !cell.option) cell.option = { id: match[1], index };
      return;
    }
    for (const match of line.replace(/`[^`]*`/g, "").matchAll(ATTRIBUTE_ID)) add(match[1], index);
  });
  for (const cell of cells.values()) {
    const label = cell.option || cell.header;
    if (label) add(label.id, label.index);
  }
  return found.sort((a, b) => a.line - b.line);
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
