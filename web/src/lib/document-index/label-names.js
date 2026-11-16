// What a label name means in each format: which files can see it, and which
// names a rename may write.

import { KINDS } from "../quarto-labels.js";

/**
 * The scope a label is visible in. A label is seen by files of its own format
 * family (LaTeX, Typst, Markdown with Quarto, HTML); an HTML fragment names
 * only its own page.
 * @param {string} format the file's format
 * @param {string} path
 * @returns {string}
 */
export function labelScope(format, path) {
  if (format === "markdown" || format === "quarto") return "markdown";
  if (format === "html") return `html:${path}`;
  return format;
}

const QUARTO_REF = new RegExp(`^(${KINDS.join("|")})-[\\w-]*\\w$`);

/**
 * Whether `name` may replace the label `oldName` in a file of this format.
 * LaTeX rejects the characters that end or break a \label argument, HTML an id
 * that would break out of its attribute, Typst anything outside a label's
 * characters, and Markdown and Quarto anything outside Pandoc's identifier
 * characters; a Quarto cross-reference label (fig-x) keeps its prefix.
 * @param {string} format
 * @param {string} oldName
 * @param {string} name
 * @returns {boolean}
 */
export function labelNameAllowed(format, oldName, name) {
  switch (format) {
    case "latex":
      return !/[{}%,\\#\s]/.test(name);
    case "html":
      return !/["'<>&#\s]/.test(name);
    case "typst":
      return /^[\p{L}\p{N}_\-:.]+$/u.test(name);
    case "markdown":
    case "quarto": {
      if (!/^[\p{L}\p{N}_\-:.]+$/u.test(name)) return false;
      const prefix = QUARTO_REF.exec(oldName)?.[1];
      return prefix === undefined || (QUARTO_REF.exec(name)?.[1] === prefix);
    }
    default:
      return !/\s/.test(name);
  }
}
