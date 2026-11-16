// The figures a document refers to, and the project path each one names.
//
// The path rule is how the real tool resolves a name, so the preview, the
// editor, visual mode and paper mode all look in the same place:
//   Markdown, Quarto, Org, HTML   relative to the main file's directory (an
//                                 include is textual, so the page being built
//                                 is the main file's page); a leading "/" is
//                                 the project root.
//   Typst                         relative to the file that writes the image;
//                                 a leading "/" is the project root.
//   LaTeX                         from the project root, then beside the file,
//                                 with graphicx's extension guessing (the
//                                 document index's own resolver).

import { dirname, normalizePath, resolvePath } from "./completion/paths.js";
import { indexFile } from "./document-index/adapters/index.js";
import { createProjectIndex } from "./document-index/project.js";
import { sourceFormatOf } from "./formats.js";
import { loadOrgSyntax, orgSyntax } from "./org-language/module.js";
import { loadTypstSyntax, typstSyntax } from "./typst-language/module.js";

/** @typedef {{ path: string, name: string, file: string, line: number }} FigureReference */

const SCHEME = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

/**
 * The project path an image named `name`, written in `file`, refers to. Null
 * for a name that is a URL. The path may be absent from the project; LaTeX
 * returns the root-relative name when no file matches.
 * @param {{ format: string, file: string, main: string, name: string, resolveFile?: (from: string, name: string, kind?: string) => string | null }} options
 * @returns {string | null}
 */
export function figurePath({ format, file, main, name, resolveFile }) {
  if (!name || SCHEME.test(name)) return null;
  if (format === "latex") return resolveFile?.(file, name, "asset") ?? normalizePath(name);
  if (name.startsWith("/")) return normalizePath(name);
  return resolvePath(dirname(format === "typst" ? file : main), name) || null;
}

/**
 * Loads the parsers the Typst and Org adapters read figures with, when the
 * project has such files. Until they load those adapters list nothing, and a
 * preview or a run can come before the editor has opened a file of that kind.
 * A parser that cannot load leaves the references empty, as before.
 * Null when the project has no such file, so a caller need not wait at all.
 * @param {{ texts?: Record<string, string> }} tree
 * @returns {Promise<unknown> | null}
 */
export function figureSyntaxReady(tree) {
  const names = Object.keys(tree?.texts ?? {});
  const typst = names.some((name) => /\.typ$/i.test(name));
  const org = names.some((name) => /\.org$/i.test(name));
  if (!typst && !org) return null;
  return Promise.all([typst ? loadTypstSyntax() : null, org ? loadOrgSyntax() : null]);
}

/** @param {string} text @returns {number[]} offsets at which each line starts */
function lineStarts(text) {
  const starts = [0];
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  return starts;
}

// referencedFigures runs on every keystroke, but most keystrokes leave the
// included files alone, so each file's index and line starts are kept keyed by
// path and exact text. Only a file whose text changed is parsed again. The
// cached index and starts are never mutated (the loop sorts a copy of
// `items`), so sharing them between calls is safe. Bounded to the 64 most
// recently used paths.
const INDEX_CACHE_LIMIT = 64;
/** @type {Map<string, { text: string, index: import("./document-index/schema.js").FileIndex | null, starts: number[] }>} */
const indexCache = new Map();

/**
 * Whether the parser the file's adapter reads with has loaded. The Typst and
 * Org adapters index nothing before it has.
 * @param {string} file
 */
function parserReady(file) {
  const format = sourceFormatOf(file);
  return (format !== "typst" || typstSyntax() !== null) && (format !== "org" || orgSyntax() !== null);
}

/**
 * The index and line starts of one file, from the cache when its text is
 * unchanged. A file is only cached once its parser is ready (parserReady);
 * otherwise a render that came first would keep an empty index.
 * @param {string} file
 * @param {string} text
 */
function indexed(file, text) {
  const hit = indexCache.get(file);
  if (hit && hit.text === text) {
    indexCache.delete(file);
    indexCache.set(file, hit);
    return hit;
  }
  const entry = { text, index: indexFile(file, text), starts: lineStarts(text) };
  if (parserReady(file)) {
    indexCache.delete(file);
    indexCache.set(file, entry);
    if (indexCache.size > INDEX_CACHE_LIMIT) indexCache.delete(indexCache.keys().next().value);
  }
  return entry;
}

/** @param {number[]} starts @param {number} offset @returns {number} 1-based line */
function lineOf(starts, offset) {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if (starts[mid] <= offset) low = mid;
    else high = mid - 1;
  }
  return low + 1;
}

/**
 * Local module paths a Typst file imports (`#import "mod.typ": x`). Package
 * imports (`@preview/...`) are skipped.
 * @param {string} text
 * @returns {string[]}
 */
function typstImports(text) {
  const out = [];
  for (const match of text.matchAll(/#import\s+"([^"\n]+)"/g)) {
    if (!match[1].startsWith("@")) out.push(match[1]);
  }
  return out;
}


// A keystroke in ordinary prose cannot change which figures and includes a
// document names, yet the full pass above indexes the whole main file (headings,
// labels, citations, everything) on every keystroke. So the last full result is
// remembered with a signature of the main text: only the lines that could
// contribute an image, an include, or the context that decides whether a later
// line counts (a fence, front matter, math, a brace or comment). Which lines
// are blank is part of the signature too: a blank line ends a paragraph, which
// decides where math and other spans stop, without touching a signed line.
// Line numbers are part of it because results carry them. While the signature,
// the known paths and every included file's text are unchanged, the result is
// too.
//
// Invariant, per format: every line the adapter reads an asset or include
// from, or whose content can change how another such line is read, contains
// one of `tokens`; a construct that continues past its first line is covered by
// `follow` (the lines after a line with one of its `tokens`).
//   Markdown, Quarto  (document-index/adapters/markdown.js)
//     "![" an image (its regex cannot span a line); "{{<", "include", "}}" a
//     Quarto include, whose parts may sit on lines of their own (follow: the
//     lines after a "{{<" line, up to the line holding ">}}" and at most 256
//     characters, so an unclosed "{{<" cannot sign the rest of the file); "`" and "~" fences and inline
//     code, which blank what they hold; "$" math, which blanks too; "---" and
//     "..." front matter, which is code.
//   LaTeX  (document-index/adapters/latex.js)
//     "\\" any command, including \includegraphics, \input and the
//     environments (verbatim) and \verb that make a command inert; "%" comments;
//     "{" "}" "[" "]" the argument groups and options; "$" math. argumentOf
//     skips up to 200 characters, options included, between the command and
//     its {path}, so a line within 256 characters after \includegraphics,
//     \input, \include or \subfile is signed as well (follow).
// Markdown also blanks math, and a math span can cross unsigned lines in a way
// a blank line does not show (a line between the delimiters that becomes a
// heading ends the paragraph and breaks the span). Math is the only blanking
// that can span unsigned lines, so a main text with math across a line break
// (a line with an odd number of unescaped "$", or of "$$") has no signature and
// takes the full pass. Fenced code is counted too, which is only conservative.
// Typst, Org and HTML have no list (their constructs span lines through
// strings, calls, comments and attributes that no small token set bounds), so
// they always take the full pass.
/** @typedef {{ tokens: string[], follow: { tokens: string[], chars: number, until?: string }, math?: boolean }} FastSpec */
/** @type {FastSpec} */
const MARKDOWN_FAST = {
  tokens: ["![", "{{<", "include", "}}", "`", "~", "$", "---", "..."],
  follow: { tokens: ["{{<"], chars: 256, until: ">}}" },
  math: true,
};
/** @type {Record<string, FastSpec>} */
const FAST = {
  markdown: MARKDOWN_FAST,
  quarto: MARKDOWN_FAST,
  latex: {
    tokens: ["\\", "%", "{", "}", "[", "]", "$"],
    follow: { tokens: ["\\include", "\\input", "\\subfile"], chars: 256 },
  },
};
/** @type {Map<FastSpec, { token: RegExp, follow: RegExp, until: RegExp | null }>} */
const fastPatterns = new Map();

/** @param {string[]} words */
const anyOf = (words) => new RegExp(words.map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"));

const UNESCAPED_DOLLAR = /(?<!\\)\$/g;
const UNESCAPED_DISPLAY = /(?<!\\)\$\$/g;
/** @param {string} line @param {RegExp} pattern */
const count = (line, pattern) => line.match(pattern)?.length ?? 0;

/**
 * The lines of `text` that matter to the fast path, each with its 0-based
 * number, in one string.
 * @param {string} text
 * @param {FastSpec} spec
 * @returns {string | null} null when the text has multi-line math and spec.math is set
 */
function signature(text, spec) {
  let patterns = fastPatterns.get(spec);
  if (!patterns) {
    patterns = { token: anyOf(spec.tokens), follow: anyOf(spec.follow.tokens), until: spec.follow.until ? anyOf([spec.follow.until]) : null };
    fastPatterns.set(spec, patterns);
  }
  const out = [];
  const blank = [];
  let reach = 0;
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    let take = patterns.token.test(line);
    if (reach > 0) {
      take = true;
      reach -= line.length + 1;
      if (patterns.until?.test(line)) reach = 0;
    }
    if (spec.math && line.includes("$") && (count(line, UNESCAPED_DOLLAR) % 2 || count(line, UNESCAPED_DISPLAY) % 2)) return null;
    if (take) out.push(`${i}:${line}`);
    if (!line.trim()) blank.push(i);
    if (patterns.follow.test(line)) reach = spec.follow.chars;
  }
  // The blank-line indices are digits and commas, set apart by a NUL.
  return `${out.join("\n")}\0${blank.join(",")}`;
}

/** @typedef {{ mainPath: string, format: string, knownKey: string, signature: string, others: Map<string, string | undefined>, result: FigureReference[] }} Memo */
/** @type {Memo | null} */
let memo = null;

/**
 * Every image reference in the main document and in the files it includes, in
 * document order, each resolved by the path rule above. `tree` is the
 * `{ main, texts, assets?, digests? }` the renderers take.
 * @param {{ main?: string, texts?: Record<string, string>, assets?: Record<string, unknown>, digests?: Record<string, string> }} tree
 * @returns {FigureReference[]}
 */
export function referencedFigures(tree) {
  const main = tree?.main ?? "";
  const texts = tree?.texts ?? {};
  if (!main || typeof texts[main] !== "string") return [];
  const known = new Set([...Object.keys(texts), ...Object.keys(tree.assets ?? {}), ...Object.keys(tree.digests ?? {})].map(normalizePath));
  const mainFormat = sourceFormatOf(main);
  const spec = FAST[mainFormat];
  const mainSignature = spec ? signature(texts[main], spec) : null;
  const fast = spec && mainSignature !== null;
  const knownKey = fast ? [...known].sort().join("\n") : "";
  if (
    memo && fast && memo.mainPath === main && memo.format === mainFormat && memo.knownKey === knownKey &&
    memo.signature === mainSignature && [...memo.others].every(([file, text]) => texts[file] === text)
  ) {
    return memo.result.map((reference) => ({ ...reference }));
  }
  const project = createProjectIndex();
  project.setFiles([...known]);
  /** @type {FigureReference[]} */
  const out = [];
  const seen = new Set();
  const queue = [main];
  let ready = true;
  while (queue.length) {
    const file = /** @type {string} */ (queue.shift());
    if (seen.has(file)) continue;
    seen.add(file);
    const text = texts[file];
    if (typeof text !== "string") continue;
    if (!parserReady(file)) ready = false;
    const { index, starts } = indexed(file, text);
    if (!index) continue;
    const format = sourceFormatOf(main) || index.format;
    if (index.format === "typst") {
      for (const name of typstImports(text)) {
        const target = project.resolveFile(file, name, "include");
        if (target && !seen.has(target)) queue.push(target);
      }
    }
    for (const item of [...index.items].sort((a, b) => a.range[0] - b.range[0])) {
      if (item.name === undefined) continue;
      if (item.kind === "asset") {
        const path = figurePath({ format, file, main, name: item.name, resolveFile: project.resolveFile });
        if (path) out.push({ path, name: item.name, file, line: lineOf(starts, item.range[0]) });
      } else if (item.kind === "include" && index.format !== "html") {
        const target = project.resolveFile(file, item.name, "include");
        if (target && !seen.has(target)) queue.push(target);
      }
    }
  }
  if (fast && ready) {
    const others = new Map([...seen].filter((file) => file !== main).map((file) => [file, texts[file]]));
    memo = { mainPath: main, format: mainFormat, knownKey, signature: /** @type {string} */ (mainSignature), others, result: out.map((reference) => ({ ...reference })) };
  }
  return out;
}
