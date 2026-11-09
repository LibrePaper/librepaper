// The typst adapter of the document index. It walks the typst-syntax parse
// tree of one source (typstTree in source-trees.js) and maps the nodes it finds
// to IndexItems: refs, cites, labels, definitions, includes, assets,
// bibliographies, code, math and blocks. Headings come from the outline and the
// spellcheck skips from skipRanges. Offsets are UTF-16 offsets into the text.
// `path` is the key the parse tree is cached under, so an edit reparses
// incrementally. Without a loaded parser the adapter returns no items.
// Synchronous and pure: the result depends only on the text.

import { extractOutline } from "../../outline.js";
import { skipRanges } from "../../spellcheck/index.js";
import { typstRefInWord } from "../../citation-keys.js";
import { typstTree } from "../../source-trees.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/**
 * A node of the typst parse tree (a Lezer SyntaxNode).
 * @typedef {any} SyntaxNode
 */

/** Children of an argument list that are not positional arguments. */
const NOT_ARGUMENT = new Set([
  "LeftParen",
  "RightParen",
  "Space",
  "Comma",
  "LineComment",
  "BlockComment",
  "Named",
]);

/**
 * The items of one typst source. Items are not sorted.
 * @param {string} text
 * @param {string} path
 * @returns {IndexItem[]}
 */
export function typstAdapter(text, path) {
  const source = String(text ?? "");
  const tree = typstTree(source, path);
  if (!tree) return [];

  const headings = headingItems(source);

  /** @type {IndexItem[]} */
  const items = [...headings];
  for (const range of skipRanges(source, "typst")) {
    items.push({ kind: "markup", range, attrs: { prose: "skip" } });
  }

  tree.iterate({
    enter(ref) {
      const node = ref.node;
      switch (node.name) {
        case "Raw":
          items.push({ kind: "code", range: [node.from, node.to] });
          return;
        case "Equation":
          items.push({ kind: "math", range: [node.from, node.to] });
          return;
        case "ContentBlock":
        case "CodeBlock":
          if (isBlock(source, node)) items.push({ kind: "block", range: [node.from + 1, node.to - 1] });
          return;
        case "Ref": {
          const marker = node.getChild("RefMarker");
          if (!marker || typstRefInWord(source, marker.from)) return;
          items.push({
            kind: "ref",
            range: [marker.from, marker.to],
            name: source.slice(marker.from + 1, marker.to),
            nameRange: [marker.from + 1, marker.to],
            namespace: "either",
          });
          return;
        }
        case "FuncCall":
          items.push(...callItems(source, node));
          return;
        case "ModuleInclude": {
          const str = positionalStrings(node)[0];
          if (str) items.push(quotedItem("include", (hashBefore(node) ?? node).from, str, source));
          return;
        }
        case "LetBinding": {
          const definition = definitionItem(source, node);
          if (definition) items.push(definition);
          return;
        }
        case "Label":
          if (node.parent?.name !== "Args") items.push(labelItem(source, headings, node));
          else {
            // A label passed to a call, as in #link(<x>) or query(<x>), refers
            // to it. cite and ref read their own label in callItems.
            const callee = node.parent.parent ? calleeName(source, node.parent.parent) : null;
            if (callee !== "cite" && callee !== "ref") {
              items.push({ kind: "ref", range: [node.from, node.to], ...innerOf(source, node), namespace: "label" });
            }
          }
          return;
        default:
          return;
      }
    },
  });

  return items;
}

/**
 * Headings from the outline scanner. A heading runs to the end of its line.
 * @param {string} source
 * @returns {IndexItem[]}
 */
function headingItems(source) {
  return extractOutline(source, "typst").map(({ title, level, from }) => {
    let end = from;
    while (end < source.length && source[end] !== "\n" && source[end] !== "\r") end++;
    return { kind: "heading", range: [from, end], level, title };
  });
}

/**
 * The `#` directly before a markup-level node, or null.
 * @param {SyntaxNode} node
 * @returns {SyntaxNode | null}
 */
function hashBefore(node) {
  const previous = node.prevSibling;
  return previous?.name === "Hash" ? previous : null;
}

/**
 * The name of a call whose callee is a plain identifier, or null.
 * @param {string} source
 * @param {SyntaxNode} call
 * @returns {string | null}
 */
function calleeName(source, call) {
  const callee = call.firstChild;
  return callee?.name === "Ident" ? source.slice(callee.from, callee.to) : null;
}

/**
 * The first positional argument of an argument list, or null.
 * @param {SyntaxNode} args
 * @returns {SyntaxNode | null}
 */
function firstArgument(args) {
  for (let child = args.firstChild; child; child = child.nextSibling) {
    if (!NOT_ARGUMENT.has(child.name)) return child;
  }
  return null;
}

/**
 * The string literals of a node that are not named arguments, looking into
 * arrays and parenthesized values.
 * @param {SyntaxNode} node
 * @param {SyntaxNode[]} [out]
 * @returns {SyntaxNode[]}
 */
function positionalStrings(node, out = []) {
  for (let child = node.firstChild; child; child = child.nextSibling) {
    if (child.name === "Str") out.push(child);
    else if (child.name === "Array" || child.name === "Parenthesized") positionalStrings(child, out);
  }
  return out;
}

/**
 * The name between the delimiters of a label or string literal.
 * @param {string} source
 * @param {SyntaxNode} node
 * @returns {{ name: string, nameRange: [number, number] }}
 */
function innerOf(source, node) {
  const nameRange = /** @type {[number, number]} */ ([node.from + 1, node.to - 1]);
  return { name: source.slice(nameRange[0], nameRange[1]), nameRange };
}

/**
 * A path-carrying item: the range runs from `from` to the end of the string
 * literal, the name is the literal's contents.
 * @param {"include" | "asset" | "bibliography"} kind
 * @param {number} from
 * @param {SyntaxNode} str
 * @param {string} source
 * @returns {IndexItem}
 */
function quotedItem(kind, from, str, source) {
  return { kind, range: [from, str.to], ...innerOf(source, str) };
}

/**
 * Whether a bracket or brace block is one of the multi-line blocks the index
 * keeps: it spans lines and its statement starts with `#`.
 * @param {string} source
 * @param {SyntaxNode} node
 * @returns {boolean}
 */
function isBlock(source, node) {
  if (!source.slice(node.from, node.to).includes("\n")) return false;
  let top = node;
  while (top.parent && top.parent.name !== "Markup" && top.parent.name !== "Code") top = top.parent;
  return top.prevSibling?.name === "Hash";
}

/**
 * Whether the position `at` is the first non-blank text of its line.
 * @param {string} source
 * @param {number} at
 * @returns {boolean}
 */
function startsLine(source, at) {
  const start = source.lastIndexOf("\n", at - 1) + 1;
  return source.slice(start, at).trim() === "";
}

/**
 * The items of one call: `#cite`, `#ref`, `image`, `bibliography`.
 * @param {string} source
 * @param {SyntaxNode} node
 * @returns {IndexItem[]}
 */
function callItems(source, node) {
  const name = calleeName(source, node);
  const args = node.getChild("Args");
  if (!name || !args) return [];
  const hash = hashBefore(node);
  const first = firstArgument(args);

  switch (name) {
    case "cite": {
      if (!hash || !first) return [];
      let key = null;
      if (first.name === "Label") key = innerOf(source, first);
      else if (first.name === "FuncCall" && calleeName(source, first) === "label") {
        const str = firstArgument(first.getChild("Args"));
        if (str?.name === "Str") key = innerOf(source, str);
      }
      if (!key) return [];
      return [{ kind: "cite", range: [hash.from, node.to], ...key, namespace: "cite" }];
    }
    case "ref": {
      if (!hash || first?.name !== "Label") return [];
      return [{ kind: "ref", range: [hash.from, node.to], ...innerOf(source, first), namespace: "label" }];
    }
    case "image": {
      if (first?.name !== "Str") return [];
      return [quotedItem("asset", (hash ?? node.firstChild).from, first, source)];
    }
    case "bibliography":
      return positionalStrings(args).map((str) => quotedItem("bibliography", str.from, str, source));
    default:
      return [];
  }
}

/**
 * A `#let` definition at the start of a markup line. `#let f(...) = ...` is a
 * function, `#let x = ...` a variable.
 * @param {string} source
 * @param {SyntaxNode} node
 * @returns {IndexItem | null}
 */
function definitionItem(source, node) {
  const hash = hashBefore(node);
  if (!hash || !startsLine(source, hash.from)) return null;
  let target = null;
  for (let child = node.firstChild; child; child = child.nextSibling) {
    if (child.name === "Ident" || child.name === "Closure") {
      target = child;
      break;
    }
  }
  if (!target) return null;
  const isFunction = target.name === "Closure";
  const ident = isFunction ? target.firstChild : target;
  if (ident?.name !== "Ident") return null;
  const nameRange = /** @type {[number, number]} */ ([ident.from, ident.to]);
  return {
    kind: "definition",
    range: [hash.from, ident.to],
    name: source.slice(ident.from, ident.to),
    nameRange,
    attrs: { type: isFunction ? "function" : "variable" },
  };
}

/**
 * A label declaration. Its title is the heading it ends, or the caption of the
 * figure it follows.
 * @param {string} source
 * @param {IndexItem[]} headings
 * @param {SyntaxNode} node
 * @returns {IndexItem}
 */
function labelItem(source, headings, node) {
  /** @type {IndexItem} */
  const item = { kind: "label", range: [node.from, node.to], ...innerOf(source, node) };
  const title = labelTitle(source, headings, node);
  if (title) item.title = title;
  return item;
}

/**
 * The title of a label: the heading title when the label ends a heading line,
 * else the caption of the `figure` call it directly follows.
 * @param {string} source
 * @param {IndexItem[]} headings
 * @param {SyntaxNode} label
 * @returns {string | null}
 */
function labelTitle(source, headings, label) {
  let before = label.prevSibling;
  while (before?.name === "Space") before = before.prevSibling;
  if (!before) return null;

  if (before.name === "Heading") {
    if (source.slice(before.to, label.from).includes("\n")) return null;
    const heading = headings.find((item) => item.range[0] <= label.from && label.from < item.range[1]);
    if (!heading) return null;
    const { name } = innerOf(source, label);
    return collapse((heading.title ?? "").split(`<${name}>`).join(""));
  }

  if (before.name === "FuncCall" && calleeName(source, before) === "figure") {
    const caption = captionOf(source, before);
    return caption ? collapse(source.slice(caption.from + 1, caption.to - 1)) : null;
  }
  return null;
}

/**
 * The content block given as the `caption:` argument of a call, or null.
 * @param {string} source
 * @param {SyntaxNode} call
 * @returns {SyntaxNode | null}
 */
function captionOf(source, call) {
  const args = call.getChild("Args");
  for (let child = args?.firstChild; child; child = child.nextSibling) {
    if (child.name !== "Named") continue;
    const key = child.firstChild;
    if (key?.name !== "Ident" || source.slice(key.from, key.to) !== "caption") continue;
    return child.getChild("ContentBlock");
  }
  return null;
}

/**
 * Whitespace collapsed and trimmed, or null when nothing is left.
 * @param {string} text
 * @returns {string | null}
 */
function collapse(text) {
  return text.replace(/\s+/g, " ").trim() || null;
}
