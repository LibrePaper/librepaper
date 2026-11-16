// The source that visual mode folds, per format. Each finder reads a document's
// text and returns the ranges to hide behind a pill: the preamble, includes,
// comments, and definitions that the editor never draws. Pure functions of the
// text, so they run on the main thread and never touch CodeMirror. The Org and
// Typst finders read a syntax tree from a wasm module that loads asynchronously:
// until it is loaded they return null, not an empty list.

import { latexCommands, readArguments } from "../latex-language/args.js";
import { m } from "../paraglide/messages.js";
import { splitBlocks } from "../paper/blocks.js";
import { ORG_HEADER_KINDS, splitOrgBlocks } from "../paper/org-blocks.js";
import { splitTypstBlocks } from "../paper/typst-blocks.js";
import { orgTree } from "../source-trees.js";

// For a preamble fold that a title block can stand for, `others` lists the header's
// lines that the title block does not show; the paper layer shows it beside the pill.
/** @typedef {{ from: number, to: number, kind: "preamble" | "include" | "comment" | "definitions", detail?: string, target?: string, others?: string }} Fold */

// Details are short and muted: one line, at most 60 characters.
const DETAIL_LIMIT = 60;

/**
 * Collapses a detail to one line of at most 60 characters, cut with "…".
 * @param {string} text
 */
function cut(text) {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > DETAIL_LIMIT ? `${flat.slice(0, DETAIL_LIMIT - 1)}…` : flat;
}

/**
 * The first line of `text` that has any content after `clean` has trimmed it.
 * @param {string} text
 * @param {(line: string) => string} clean
 */
function firstContent(text, clean) {
  for (const line of text.split(/\r\n|\r|\n/)) {
    const content = clean(line).trim();
    if (content) return content;
  }
  return "";
}

/**
 * Wraps a finder so that it returns the same array for the same text, as the
 * block splitters do: the last text is remembered. A null result means the
 * finder is not ready (its syntax module is still loading): the caller gets
 * [], and nothing is remembered, so the next call computes again.
 * @param {(text: string) => Fold[] | null} compute
 * @returns {(text: string) => Fold[]}
 */
function memoize(compute) {
  /** @type {{ text: string, folds: Fold[] } | null} */
  let last = null;
  return (text) => {
    if (last && last.text === text) return last.folds;
    const folds = compute(text);
    if (folds === null) return [];
    last = { text, folds };
    return folds;
  };
}

// ---- LaTeX ------------------------------------------------------------------

// The preamble is looked for in the first lines only, as the editor does.
const SEARCH_LINES = 2000;

/**
 * The lines of a source, each with its offsets. A line ends at a line break or
 * the text's end; `to` is the offset of that break.
 * @param {string} text
 * @returns {{ from: number, to: number, text: string }[]}
 */
function linesOf(text) {
  /** @type {{ from: number, to: number, text: string }[]} */
  const lines = [];
  const breaks = /\r\n|\r|\n/g;
  let from = 0;
  for (let match = breaks.exec(text); match; match = breaks.exec(text)) {
    lines.push({ from, to: match.index, text: text.slice(from, match.index) });
    from = breaks.lastIndex;
  }
  lines.push({ from, to: text.length, text: text.slice(from) });
  return lines;
}

/**
 * The preamble: from the start of the document to the line before the first
 * uncommented `\begin{document}`. Null when there is none, or when the text
 * before it names no `\documentclass`.
 * @param {string} text
 * @param {{ from: number, to: number, text: string }[]} lines
 * @returns {{ to: number, detail: string, next: number } | null} `next` is the index of the `\begin{document}` line
 */
function findLatexPreamble(text, lines) {
  const last = Math.min(lines.length, SEARCH_LINES);
  for (let index = 0; index < last; index++) {
    const at = lines[index].text.indexOf("\\begin{document}");
    // A commented-out \begin{document} does not end the preamble.
    if (at === -1 || lines[index].text.slice(0, at).includes("%")) continue;
    if (index === 0) return null;
    const to = lines[index - 1].to;
    const { code, commands } = latexCommands(text.slice(0, to));
    if (!commands.some((command) => command.type === "command" && command.name === "documentclass")) return null;
    return { to, detail: summariseLatex(code, commands), next: index };
  }
  return null;
}

/**
 * The metadata beside the "Preamble" label: the class when known, the package
 * count when not zero, and nothing when neither is known.
 * @param {string} code
 * @param {ReturnType<typeof latexCommands>["commands"]} commands
 */
function summariseLatex(code, commands) {
  let documentClass = null;
  let packages = 0;
  for (const command of commands) {
    if (command.type !== "command" || (command.name !== "documentclass" && command.name !== "usepackage")) continue;
    const args = readArguments(code, command.to);
    const named = args.find((arg) => arg.kind === "group");
    if (command.name === "documentclass" && documentClass === null) {
      documentClass = named ? named.value : "";
    } else if (command.name === "usepackage" && named) {
      packages += named.value.split(",").filter((name) => name.trim() !== "").length;
    }
  }
  if (documentClass !== null && documentClass.trim() !== "") {
    return packages > 0 ? m.preamble_meta_class({ documentClass: documentClass.trim(), count: packages }) : documentClass.trim();
  }
  return packages > 0 ? m.preamble_meta_packages({ count: packages }) : "";
}

/**
 * The text of a line with its trailing `%` comment removed. A backslash
 * escapes the character after it, so `\%` and `\\%` are read correctly.
 * @param {string} line
 */
function withoutComment(line) {
  for (let i = 0; i < line.length; i++) {
    if (line[i] === "\\") i++;
    else if (line[i] === "%") return line.slice(0, i);
  }
  return line;
}

/** @param {string} text @returns {Fold[]} */
function latexFoldsOf(text) {
  const lines = linesOf(text);
  const preamble = findLatexPreamble(text, lines);
  /** @type {Fold[]} */
  const folds = [];
  if (preamble) folds.push({ from: 0, to: preamble.to, kind: "preamble", detail: cut(preamble.detail) });

  // Which lines are inside a verbatim-like environment, including its tags.
  const verbatim = new Array(lines.length).fill(false);
  let environment = null;
  lines.forEach((line, index) => {
    if (environment) verbatim[index] = true;
    const begin = /\\begin\{(verbatim|lstlisting|minted)\*?\}/.exec(line.text);
    if (!environment && begin) {
      environment = begin[1];
      verbatim[index] = true;
    }
    if (environment && line.text.includes(`\\end{${environment}`)) environment = null;
  });

  // Folds are found after the preamble, which is the whole text when there is none.
  for (let index = preamble ? preamble.next : 0; index < lines.length; index++) {
    if (verbatim[index]) continue;
    const line = lines[index];
    const body = withoutComment(line.text).trim();
    const include = /^\\(input|include|subfile)\{([^{}]*)\}$/.exec(body);
    const name = include?.[2].trim();
    if (include && name) {
      folds.push({ from: line.from, to: line.to, kind: "include", target: name, detail: cut(name) });
      continue;
    }
    if (!line.text.trim().startsWith("%")) continue;
    // A run of comment lines is one fold.
    let end = index;
    while (end + 1 < lines.length && !verbatim[end + 1] && lines[end + 1].text.trim().startsWith("%")) end++;
    const detail = firstContent(line.text, (text) => text.trim().replace(/^%+/, ""));
    folds.push({ from: line.from, to: lines[end].to, kind: "comment", detail: cut(detail) });
    index = end;
  }
  return folds;
}

export const latexFolds = memoize(latexFoldsOf);

// ---- Markdown and Quarto ----------------------------------------------------

// A paragraph that is one `{{< include name >}}` shortcode.
const SHORTCODE_INCLUDE = /^\{\{<\s*include\s+(["']?)([^"'\s>]+)\1\s*>\}\}$/;

/**
 * The value of a top-level `title:` line in a front matter, quotes stripped.
 * @param {string} frontMatter
 */
function frontMatterTitle(frontMatter) {
  const match = /^title:[ \t]*(.*)$/m.exec(frontMatter);
  if (!match) return "";
  const value = match[1].trim();
  const quoted = /^(["'])(.*)\1$/.exec(value);
  return (quoted ? quoted[2] : value).trim();
}

// The front matter keys that a title block shows.
const TITLE_BLOCK_KEYS = new Set([
  "title",
  "subtitle",
  "author",
  "authors",
  "affiliations",
  "date",
  "abstract",
  "keywords",
  "keyword",
  "doi",
]);

/**
 * The top-level keys of a front matter that a title block does not show, each
 * once, in order, uncut.
 * @param {string} frontMatter
 * @returns {string[]}
 */
function frontMatterOtherKeys(frontMatter) {
  /** @type {string[]} */
  const keys = [];
  for (const line of frontMatter.split(/\r\n|\r|\n/)) {
    const key = /^([A-Za-z_][\w-]*)[ \t]*:/.exec(line)?.[1];
    if (key && !TITLE_BLOCK_KEYS.has(key) && !keys.includes(key)) keys.push(key);
  }
  return keys;
}

/**
 * The keys of a front matter that a title block does not show, as a detail:
 * "bibliography · format".
 * @param {string} frontMatter
 */
function frontMatterOthers(frontMatter) {
  return cut(frontMatterOtherKeys(frontMatter).join(" · "));
}

/**
 * The detail of a front matter fold: its title, then the keys that a title
 * block does not show, as "T · bibliography · format".
 * @param {string} frontMatter
 */
function frontMatterDetail(frontMatter) {
  return cut([frontMatterTitle(frontMatter), ...frontMatterOtherKeys(frontMatter)].filter(Boolean).join(" · "));
}

/** @param {string} text @returns {Fold[]} */
function markdownFoldsOf(text) {
  /** @type {Fold[]} */
  const folds = [];
  for (const block of splitBlocks(text)) {
    const source = text.slice(block.from, block.to);
    const kind = block.kind;
    if (kind === "FrontMatter") {
      folds.push({
        from: block.from,
        to: block.to,
        kind: "preamble",
        detail: frontMatterDetail(source),
        others: frontMatterOthers(source),
      });
    } else if (kind.includes("LinkReference")) {
      const label = /^\s*\[([^\]]+)\]/.exec(source)?.[1] ?? "";
      folds.push({ from: block.from, to: block.to, kind: "definitions", detail: cut(label ? `[${label}]` : "") });
    } else if (kind.includes("FootnoteDefinition")) {
      const label = /^\s*\[\^([^\]]+)\]/.exec(source)?.[1] ?? "";
      folds.push({ from: block.from, to: block.to, kind: "definitions", detail: cut(label ? `[^${label}]` : "") });
    } else if (kind.includes("CommentBlock") || (kind === "HTMLBlock" && /^ {0,3}<!--/.test(source))) {
      const body = source.replace(/^\s*<!--/, "").replace(/-->\s*$/, "");
      folds.push({ from: block.from, to: block.to, kind: "comment", detail: cut(firstContent(body, (line) => line)) });
    } else if (kind === "Paragraph") {
      const shortcode = SHORTCODE_INCLUDE.exec(source.trim());
      if (shortcode) {
        folds.push({ from: block.from, to: block.to, kind: "include", target: shortcode[2], detail: cut(shortcode[2]) });
      }
    }
  }
  return folds;
}

export const markdownFolds = memoize(markdownFoldsOf);

// ---- Org --------------------------------------------------------------------

/**
 * Every keyword of the document, with its key and value as written. Empty while
 * the Org syntax module is not loaded.
 * @param {string} text
 * @returns {{ from: number, key: string, value: string }[]}
 */
function keywordsOf(text) {
  const tree = orgTree(text);
  /** @type {{ from: number, key: string, value: string }[]} */
  const keywords = [];
  tree?.iterate({
    enter(ref) {
      if (ref.name !== "Keyword") return;
      const key = ref.node.getChild("KeywordKey");
      const value = ref.node.getChild("KeywordValue");
      keywords.push({
        from: ref.from,
        key: key ? text.slice(key.from, key.to).trim().toLowerCase() : "",
        value: value ? text.slice(value.from, value.to) : "",
      });
    },
  });
  return keywords;
}

/**
 * The file an `#+INCLUDE:` or `#+SETUPFILE:` names: the first argument,
 * optionally in double quotes, without a `::search` suffix. Null for a URL or
 * an empty argument. The same reading as orgNames in opening.js.
 * @param {string} value
 */
function orgIncludeTarget(value) {
  const target = value.startsWith('"') ? value.slice(1).split('"')[0] : value.split(/\s+/)[0];
  const file = (target ?? "").split("::")[0].trim();
  return file && !file.includes("://") ? file : null;
}

// The Org keywords that a title block shows.
const TITLE_BLOCK_KEYS_ORG = new Set(["title", "subtitle", "author", "date"]);

/**
 * The folds of an Org source, or null while the Org syntax module is not loaded.
 * @param {string} text
 * @returns {Fold[] | null}
 */
function orgFoldsOf(text) {
  const blocks = splitOrgBlocks(text);
  if (!blocks) return null;
  const keywords = keywordsOf(text);

  // The leading run: the blocks before the first one that is not a header block.
  let leading = 0;
  while (leading < blocks.length && ORG_HEADER_KINDS.has(blocks[leading].kind)) leading++;

  /** @type {Fold[]} */
  const folds = [];
  // The keys each header fold names, in order, so that the fold's detail can grow with it.
  /** @type {Map<Fold, string[]>} */
  const headerKeys = new Map();
  blocks.forEach((block, index) => {
    const source = text.slice(block.from, block.to);
    const firstLine = source.split(/\r\n|\r|\n/)[0];
    const kind = block.kind;
    const keyword = keywords.find((item) => item.from >= block.from && item.from < block.to);

    if (kind === "Keyword") {
      const key = keyword ? keyword.key : (/^\s*#\+([^:\s[]+)/.exec(firstLine)?.[1] ?? "").toLowerCase();
      if (key === "include" || key === "setupfile") {
        const target = orgIncludeTarget(keyword ? keyword.value : "");
        if (target !== null) {
          folds.push({ from: block.from, to: block.to, kind: "include", target, detail: cut(target) });
          return;
        }
      }
      if (index < leading) {
        // The header is one fold: it grows over the keywords and comments of the leading run,
        // and its detail names the keywords it hides.
        const previous = folds.at(-1);
        let header = previous && headerKeys.has(previous) ? previous : null;
        if (!header) {
          header = { from: block.from, to: block.to, kind: "preamble", detail: "" };
          folds.push(header);
          headerKeys.set(header, []);
        }
        header.to = block.to;
        const keys = headerKeys.get(header) ?? [];
        const name = `#+${key.split("[")[0].toUpperCase()}`;
        if (!keys.includes(name)) keys.push(name);
        headerKeys.set(header, keys);
        header.detail = cut(keys.join(" · "));
      } else {
        folds.push({ from: block.from, to: block.to, kind: "definitions", detail: cut(`#+${key.toUpperCase()}`) });
      }
    } else if (kind === "TitleBlock") {
      // The title block stands for the header's title, author and date. Its pill names the
      // title it shows and the other keys, or, when it shows neither, the keys it hides.
      const inside = keywords.filter((item) => item.from >= block.from && item.from < block.to);
      const title = inside.find((item) => item.key === "title");
      const titleText = title ? title.value.trim() : "";
      const keys = [...new Set(inside.map((item) => item.key.split("[")[0]))];
      const others = keys.filter((key) => !TITLE_BLOCK_KEYS_ORG.has(key)).map((key) => `#+${key.toUpperCase()}`);
      const otherText = others.join(" · ");
      const shown = [titleText, otherText].filter(Boolean).join(" · ");
      folds.push({
        from: block.from,
        to: block.to,
        kind: "preamble",
        detail: cut(shown || keys.map((key) => `#+${key.toUpperCase()}`).join(" · ")),
        others: cut(otherText),
      });
    } else if (kind.includes("Drawer")) {
      const name = /^\s*:([^:\s]+):/.exec(firstLine)?.[1] ?? kind;
      folds.push({ from: block.from, to: block.to, kind: "definitions", detail: cut(name) });
    } else if (kind === "Planning") {
      folds.push({ from: block.from, to: block.to, kind: "definitions", detail: "Planning" });
    } else if (kind === "FootnoteDefinition") {
      const label = /^\s*\[fn:([^\]]+)\]/.exec(firstLine)?.[1];
      folds.push({ from: block.from, to: block.to, kind: "definitions", detail: cut(label ? `[fn:${label}]` : kind) });
    } else if (kind === "Comment" || kind === "CommentBlock") {
      // A comment between header keywords belongs to the header.
      const previous = folds.at(-1);
      if (index < leading && previous?.kind === "preamble" && blocks.slice(index + 1, leading).some((next) => next.kind === "Keyword")) {
        previous.to = block.to;
        return;
      }
      const detail = firstContent(source, (line) => line.trim().replace(/^#\+(begin|end)_comment\b/i, "").replace(/^#\s?/, ""));
      folds.push({ from: block.from, to: block.to, kind: "comment", detail: cut(detail) });
    }
  });
  return folds;
}

export const orgFolds = memoize(orgFoldsOf);

// ---- Typst ------------------------------------------------------------------

// The rules that set a document up: they lead the document, and a later one is a definition.
const PREAMBLE_KINDS = new Set(["SetRule", "ShowRule", "LetBinding", "ModuleImport"]);

// The directive each block kind is written with.
const DIRECTIVES = { SetRule: "#set", ShowRule: "#show", LetBinding: "#let", ModuleImport: "#import", ModuleInclude: "#include" };

/**
 * The distinct directives of some blocks, in order: "#import · #set · #show".
 * @param {{ kind: string }[]} blocks
 */
function directivesOf(blocks) {
  const seen = [];
  for (const block of blocks) {
    const directive = DIRECTIVES[/** @type {keyof typeof DIRECTIVES} */ (block.kind)];
    if (directive && !seen.includes(directive)) seen.push(directive);
  }
  return seen.join(" · ");
}

/**
 * The folds of a Typst source, or null while typst-syntax.wasm is not loaded.
 * @param {string} text
 * @returns {Fold[] | null}
 */
function typstFoldsOf(text) {
  const blocks = splitTypstBlocks(text);
  if (!blocks) return null;

  // The leading run: setup rules and comments from the start of the document.
  let run = 0;
  while (run < blocks.length && (PREAMBLE_KINDS.has(blocks[run].kind) || blocks[run].kind === "Comment")) run++;
  let last = -1;
  for (let index = 0; index < run; index++) if (PREAMBLE_KINDS.has(blocks[index].kind)) last = index;

  /** @type {Fold[]} */
  const folds = [];
  if (last >= 0) {
    const detail = directivesOf(blocks.slice(0, last + 1));
    // When the paper layer draws the template's title under this pill, the pill still names the rules it hides.
    folds.push({ from: blocks[0].from, to: blocks[last].to, kind: "preamble", detail: cut(detail), others: cut(detail) });
  }

  // Comments inside the leading run up to its last rule belong to the preamble fold.
  for (let index = last + 1; index < blocks.length; index++) {
    const block = blocks[index];
    const source = text.slice(block.from, block.to);
    if (block.kind === "ModuleInclude") {
      const target = /#include\s*"([^"]*)"/.exec(source)?.[1];
      if (target !== undefined) folds.push({ from: block.from, to: block.to, kind: "include", target, detail: cut(target) });
      else folds.push({ from: block.from, to: block.to, kind: "definitions", detail: "#include" });
    } else if (PREAMBLE_KINDS.has(block.kind)) {
      folds.push({ from: block.from, to: block.to, kind: "definitions", detail: cut(directivesOf([block])) });
    } else if (block.kind === "Comment") {
      const detail = firstContent(source, (line) => line.trim().replace(/^\/\/+/, "").replace(/^\/\*+/, "").replace(/\*\/$/, ""));
      folds.push({ from: block.from, to: block.to, kind: "comment", detail: cut(detail) });
    }
  }
  return folds;
}

export const typstFolds = memoize(typstFoldsOf);
