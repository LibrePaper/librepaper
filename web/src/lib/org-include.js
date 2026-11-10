// Org `#+INCLUDE` expanded on the JavaScript side. The Org preview renderer
// drops an include line without reading the file it names, and Pandoc reads
// one text at a time, so an included section is missing from the preview and a
// heading link into it is unresolved in the PDF. This splices the included text
// into the main text before either sees it, the same approach the Quarto
// engine takes with `{{< include >}}`. Pure: no DOM and no messages, so it runs
// in workers too.

import { orgTree } from "./source-trees.js";

/// How deep includes may nest. An include past this depth is dropped.
const MAX_DEPTH = 8;
/// Nodes whose text is raw: a keyword or headline inside one is not an Org keyword.
const RAW_NODES = new Set(["Block", "Drawer", "PropertyDrawer"]);
/// Block kinds an include can wrap its file in instead of inlining it.
const BLOCKS = new Set(["src", "example", "export"]);

/** @typedef {{ main: string, texts?: Record<string, string>, assets?: Record<string, unknown> }} Tree */
/** @typedef {{ from: number, to: number, insert: string }} Edit */

/** Every node below `node`, in document order. @param {any} node */
function* descendants(node) {
  if (!node) return;
  for (let child = node.firstChild; child; child = child.nextSibling) {
    yield child;
    yield* descendants(child);
  }
}

/** Whether a node sits inside raw text (a block, a drawer). @param {any} node */
function inRaw(node) {
  for (let parent = node.parent; parent; parent = parent.parent) {
    if (RAW_NODES.has(parent.name)) return true;
  }
  return false;
}

/** The folder of a project path, "" at the root. @param {string} path */
function dirOf(path) {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

/**
 * The project path an include names, resolved against the folder of the file
 * that includes it. `.` and `..` are folded; a leading `/` is taken from the
 * project root.
 * @param {string} from path of the including file
 * @param {string} target the path as written
 */
function resolvePath(from, target) {
  const parts = target.startsWith("/") ? [] : dirOf(from).split("/").filter(Boolean);
  for (const part of target.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

/**
 * The `#+INCLUDE` keywords at the top level of `text`, with their value text.
 * Found on the Org syntax tree, never by scanning the characters.
 * @param {string} text
 * @returns {{ node: any, value: string }[]}
 */
function includeKeywords(text) {
  const found = [];
  for (const node of descendants(orgTree(text)?.topNode)) {
    if (node.name !== "Keyword" || inRaw(node)) continue;
    const key = node.getChild("KeywordKey");
    if (!key) continue;
    const name = text.slice(key.from, key.to).replace(/\[.*$/, "").trim().toUpperCase();
    if (name !== "INCLUDE") continue;
    const value = node.getChild("KeywordValue");
    found.push({ node, value: value ? text.slice(value.from, value.to).trim() : "" });
  }
  return found;
}

/**
 * The file and options of an include value: `"path" [src|example|export [arg]] [:opt value]...`.
 * The path may be quoted or bare. Returns null when there is no path.
 * @param {string} value
 */
function parseInclude(value) {
  const words = [...value.matchAll(/"([^"]*)"|(\S+)/g)].map((match) => match[1] ?? match[2]);
  if (words.length === 0) return null;
  const file = words[0];
  /** @type {Record<string, string>} */
  const options = {};
  /** @type {{ kind: string, arg: string } | null} */
  let block = null;
  for (let i = 1; i < words.length; i += 1) {
    const word = words[i];
    const next = words[i + 1];
    if (word.startsWith(":")) {
      const name = word.slice(1).toLowerCase();
      if (next !== undefined && !next.startsWith(":")) {
        options[name] = next;
        i += 1;
      } else {
        options[name] = "";
      }
      continue;
    }
    const kind = word.toLowerCase();
    if (!block && BLOCKS.has(kind)) {
      // example takes no argument; src takes a language and export a backend.
      let arg = "";
      if (kind !== "example" && next !== undefined && !next.startsWith(":")) {
        arg = next;
        i += 1;
      }
      block = { kind, arg };
    }
  }
  return { file, options, block };
}

/**
 * The lines `from` to `to` (1-based, inclusive) of `text`, from a `:lines`
 * value such as `5-10`, `5-` or `-10`. Each line keeps its newline. A value
 * that is not a range leaves the text whole.
 * @param {string} text
 * @param {string | undefined} range
 */
function selectLines(text, range) {
  if (range === undefined) return text;
  const match = /^(\d*)-(\d*)$/.exec(range.trim());
  if (!match || (!match[1] && !match[2])) return text;
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  const first = match[1] ? Math.max(1, Number(match[1])) : 1;
  const last = match[2] ? Number(match[2]) : lines.length;
  const picked = lines.slice(first - 1, last);
  return picked.length ? `${picked.join("\n")}\n` : "";
}

/**
 * Wraps a file's text in the block an include asks for. The text of a src,
 * example or export block is not Org, so it is not expanded. Example lines
 * that begin with `*`, `#+` or `,` are escaped with a comma, as Org does.
 * @param {{ kind: string, arg: string }} block
 * @param {string} text
 */
function wrapBlock(block, text) {
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  if (block.kind === "example") {
    return ["#+begin_example", ...lines.map((line) => line.replace(/^(\*|#\+|,)/, ",$1")), "#+end_example"];
  }
  if (block.kind === "export") {
    return [`#+begin_export${block.arg ? ` ${block.arg}` : ""}`, ...lines, "#+end_export"];
  }
  return [`#+begin_src${block.arg ? ` ${block.arg}` : ""}`, ...lines, "#+end_src"];
}

/**
 * Shifts the headlines of `text` so the shallowest one has `level` stars. The
 * others keep their depth relative to it; no headline goes above one star.
 * @param {string} text
 * @param {number} level
 */
function shiftHeadlines(text, level) {
  /** @type {{ from: number, stars: number }[]} */
  const headlines = [];
  for (const node of descendants(orgTree(text)?.topNode)) {
    if (node.name !== "Headline" || inRaw(node)) continue;
    const stars = /^\*+/.exec(text.slice(node.from, node.to));
    if (stars) headlines.push({ from: node.from, stars: stars[0].length });
  }
  if (headlines.length === 0) return text;
  const shift = level - Math.min(...headlines.map((headline) => headline.stars));
  if (shift === 0) return text;
  let out = text;
  for (const headline of [...headlines].sort((a, b) => b.from - a.from)) {
    const stars = "*".repeat(Math.max(1, headline.stars + shift));
    out = out.slice(0, headline.from) + stars + out.slice(headline.from + headline.stars);
  }
  return out;
}

/** @param {string} text */
function withNewline(text) {
  return text === "" || text.endsWith("\n") ? text : `${text}\n`;
}

/**
 * The text an include line becomes. Undefined leaves the line as it is (no
 * such file); an empty string removes it (the file is already being expanded,
 * or the depth limit is reached).
 * @param {Record<string, string>} texts
 * @param {string} from path of the file holding the include
 * @param {string} value the include's value text
 * @param {string[]} stack paths being expanded, outermost first
 * @param {number} depth how deeply the including file is nested
 * @returns {string | undefined}
 */
function includeBody(texts, from, value, stack, depth) {
  const spec = parseInclude(value);
  if (!spec) return undefined;
  const target = resolvePath(from, spec.file);
  if (typeof texts[target] !== "string") return undefined;
  if (stack.includes(target) || depth + 1 > MAX_DEPTH) return "";
  const selected = selectLines(texts[target], spec.options.lines);
  if (spec.block) return `${wrapBlock(spec.block, selected).join("\n")}\n`;
  let body = expandIn(texts, target, selected, [...stack, target], depth + 1);
  const level = Number(spec.options.minlevel);
  if (Number.isInteger(level) && level >= 1) body = shiftHeadlines(body, level);
  return withNewline(body);
}

/**
 * `text`, the Org text of `path`, with its include lines replaced.
 * @param {Record<string, string>} texts
 * @param {string} path
 * @param {string} text
 * @param {string[]} stack
 * @param {number} depth
 */
function expandIn(texts, path, text, stack, depth) {
  /** @type {Edit[]} */
  const edits = [];
  for (const { node, value } of includeKeywords(text)) {
    const insert = includeBody(texts, path, value, stack, depth);
    if (insert === undefined) continue;
    const from = text.lastIndexOf("\n", node.from - 1) + 1;
    const newline = text.indexOf("\n", node.from);
    edits.push({ from, to: newline < 0 ? text.length : newline + 1, insert });
  }
  let out = text;
  for (const edit of edits.sort((a, b) => b.from - a.from)) {
    out = out.slice(0, edit.from) + edit.insert + out.slice(edit.to);
  }
  return out;
}

/**
 * The tree with every top-level `#+INCLUDE` of its main text replaced by the
 * included text, recursively. Options: `:lines` selects a line range, and
 * `:minlevel` shifts the included headlines. `src`, `example` and `export`
 * forms wrap the file in the matching block. A missing file leaves its line
 * as it is; an include of a file already being expanded, or nested past the
 * depth limit, is removed. Other texts and assets are kept as they are.
 * @template {Tree} T
 * @param {T} tree
 * @returns {T}
 */
export function expandOrgIncludes(tree) {
  const texts = tree.texts || {};
  const main = tree.main;
  const source = texts[main];
  if (typeof source !== "string") return tree;
  const text = expandIn(texts, main, source, [main], 0);
  return { ...tree, texts: { ...texts, [main]: text } };
}
