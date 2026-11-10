// Completion for Org sources: citation keys, cross-reference targets, file
// paths, block names and keywords. Declines unless the edited file is Org.
import { orgMasked } from "../citation-keys.js";
import { bibliographyCompletion } from "../bibliography.js";
import { dirname, normalizePath, pathOptions } from "./paths.js";

const ORG_PATH = /\.org$/i;
// Each pattern runs on the line before the cursor, with code and comments blanked.
const FILE_LINK = /\[\[file:([^[\]\n]*)$/;
const RELATIVE_LINK = /\[\[(\.\.?\/[^[\]\n]*)$/;
const LINK_TARGET = /\[\[([^[\]\n]*)$/;
const INCLUDE = /^\s*#\+INCLUDE:\s*"([^"\n]*)$/i;
const BIBLIOGRAPHY = /^\s*#\+BIBLIOGRAPHY:[ \t]*"?([^"\n]*)$/i;
const BLOCK = /^\s*#\+BEGIN_(\w*)$/i;
const KEYWORD = /^\s*#\+(\w*)$/;
const TARGET = /<<([^<>\s]+)>>/g;
const NAME = /^\s*#\+NAME:\s*(\S+)/i;
const CUSTOM_ID = /^\s*:CUSTOM_ID:\s*(\S+)/i;
const HEADING = /^\*+\s+(.*?)\s*$/;

const BLOCKS = ["SRC", "QUOTE", "EXAMPLE", "VERSE", "CENTER", "COMMENT", "EXPORT"];
const KEYWORDS = ["TITLE:", "AUTHOR:", "DATE:", "CAPTION:", "NAME:", "OPTIONS:", "INCLUDE:", "BIBLIOGRAPHY:", "LANGUAGE:", "TOC:"];

function result(from, validFor, options) {
  return options.length ? { from, validFor, options } : null;
}

/**
 * Cross-reference targets of Org texts: `<<target>>` and `#+NAME:` labels, and
 * `:CUSTOM_ID:` headings, which are referred to as `#id`. The file `first`
 * comes first; one option per name.
 * @param {Record<string, string | null | undefined>} texts
 * @param {string} [first]
 * @returns {Array<{ label: string, type: string, detail: string, info?: string }>}
 */
export function orgLabels(texts = {}, first = "") {
  const paths = Object.keys(texts).filter((path) => ORG_PATH.test(path)).sort();
  const ordered = first && paths.includes(first) ? [first, ...paths.filter((path) => path !== first)] : paths;
  const seen = new Set();
  const labels = [];
  const add = (label, type, path, line, info) => {
    if (seen.has(label)) return;
    seen.add(label);
    labels.push({ label, type, detail: `${path}:${line}`, ...(info ? { info } : {}) });
  };
  for (const path of ordered) {
    const lines = orgMasked(String(texts[path] ?? "")).split("\n");
    let title = "";
    lines.forEach((line, index) => {
      const heading = line.match(HEADING);
      if (heading) title = heading[1];
      const number = index + 1;
      for (const target of line.matchAll(TARGET)) add(target[1], "variable", path, number, "");
      const name = line.match(NAME);
      if (name) add(name[1], "variable", path, number, "");
      const id = line.match(CUSTOM_ID);
      if (id) add(`#${id[1]}`, "heading", path, number, title);
    });
  }
  return labels;
}

/** Completion source for Org. Returns null outside Org files. */
export function orgCompletion(project) {
  return (context) => {
    const current = project();
    if (current.format !== "org") return null;
    const { doc } = context.state;
    const line = doc.lineAt(context.pos);
    const rawBefore = line.text.slice(0, context.pos - line.from);
    if (!/\[\[|#\+|@/.test(rawBefore)) return null;

    // Citation keys: the bibliography source, when the project supplies entries.
    const cited = bibliographyCompletion({ entries: () => current.entries, format: "org" })(context);
    if (cited) return cited;

    // Everything else is read from the masked text, so code and comments offer nothing.
    const text = doc.toString();
    const masked = orgMasked(text);
    const lineStart = masked.lastIndexOf("\n", context.pos - 1) + 1;
    const before = masked.slice(lineStart, context.pos);
    const directory = dirname(current.path);
    let match;

    if ((match = before.match(FILE_LINK))) {
      return result(context.pos - match[1].length, /^[^[\]\n]*$/, pathOptions(current.files, directory));
    }
    if ((match = before.match(RELATIVE_LINK))) {
      // "./" is kept in front of each path that does not climb out of the folder.
      const options = pathOptions(current.files, directory).map((option) =>
        option.label.startsWith("../") ? option : { ...option, label: `./${option.label}` });
      return result(context.pos - match[1].length, /^[^[\]\n]*$/, options);
    }
    if ((match = before.match(INCLUDE))) {
      return result(context.pos - match[1].length, /^[^"\n]*$/, pathOptions(current.files, directory));
    }
    if ((match = before.match(BIBLIOGRAPHY))) {
      const options = pathOptions(current.files, directory, { extensions: ["bib"] });
      return result(context.pos - match[1].length, /^[^"\n]*$/, options);
    }
    if ((match = before.match(BLOCK))) {
      const options = BLOCKS.map((label) => ({ label, type: "keyword", detail: "" }));
      return result(context.pos - match[1].length, /^\w*$/, options);
    }
    if ((match = before.match(KEYWORD))) {
      const options = KEYWORDS.map((label) => ({ label, type: "keyword", detail: "" }));
      return result(context.pos - match[1].length, /^\w*$/, options);
    }
    if ((match = before.match(LINK_TARGET))) {
      const texts = { ...current.texts, [normalizePath(current.path)]: text };
      return result(context.pos - match[1].length, /^[^[\]\n]*$/, orgLabels(texts, normalizePath(current.path)));
    }
    return null;
  };
}
