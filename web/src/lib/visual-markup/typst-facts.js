// The facts of a Typst project that the visual layer reads, in the shape of
// documentFacts (facts.js): the numbers of headings, figures and display
// equations, the labels bound to them, the citations and the citation style.
// It is one pass over each file in include order, over the typst-syntax parse
// tree of the file (typstTree in source-trees.js). Comments, raw text and math
// are nodes of that tree, so nothing in them reads as a label or a citation.
// While typst-syntax.wasm is not loaded there is no tree, and the facts are
// those of an empty project: the product has no regex fallback for Typst.

import { roman } from "./labels.js";
import { rankEntries } from "../bibliography.js";
import { citationFacts } from "./facts.js";
import { typstRefInWord } from "../citation-keys.js";
import { typstTree } from "../source-trees.js";

// Counting symbols of a numbering pattern. Other characters are literal.
const SYMBOLS = "1aAiI";
const TRIM = /^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu;
const KIND_NAMES = new Map([
  ["image", "figure"],
  ["table", "table"],
  ["raw", "listing"],
]);
const SET_TARGETS = new Set(["heading", "figure", "math.equation", "cite"]);
const TRIVIA = new Set(["Space", "LineComment", "BlockComment"]);
const NOT_ARGUMENT = new Set(["LeftParen", "RightParen", "Comma", ...TRIVIA]);
const BLANK_GAP = /^[ \t]*$/;
const REST_OF_LINE = /[ \t]*(?:\r?\n|$)/y;

// Styles that cite as author and year. The others, the numeric ones, are the
// default, as Typst's own default style (IEEE) is numeric.
const AUTHOR_YEAR = /^(?:apa|american-psychological|chicago|harvard|mla|modern-language|elsevier-harvard|council-of-science|american-sociological)|author-(?:date|year)/;

// Joins and resolves . and .. so that paths compare as project paths.
function normalize(path) {
  const parts = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

function folderOf(path) {
  return path.slice(0, path.lastIndexOf("/") + 1);
}

// The `#` directly before a markup-level node, or null.
function hashBefore(node) {
  const previous = node.prevSibling;
  return previous?.name === "Hash" ? previous : null;
}

// The name of a call whose callee is a plain identifier, or null.
function calleeName(source, call) {
  const callee = call.firstChild;
  return callee?.name === "Ident" ? source.slice(callee.from, callee.to) : null;
}

// The text between the delimiters of a label or string literal.
const innerOf = (source, node) => source.slice(node.from + 1, node.to - 1);

// The first child of an argument list that is an argument, or null.
function firstArgument(args) {
  for (let child = args.firstChild; child; child = child.nextSibling) {
    if (!NOT_ARGUMENT.has(child.name)) return child;
  }
  return null;
}

// The value node of the named argument `name` of an argument list. Undefined
// when the argument is absent, null when it has no value node.
function namedArg(source, args, name) {
  for (let child = args?.firstChild; child; child = child.nextSibling) {
    if (child.name !== "Named") continue;
    const key = child.firstChild;
    if (key?.name !== "Ident" || source.slice(key.from, key.to) !== name) continue;
    let value = child.lastChild;
    while (value && TRIVIA.has(value.name)) value = value.prevSibling;
    return value && value !== key && value.name !== "Colon" ? value : null;
  }
  return undefined;
}

// The value of `numbering:` in an argument list. Undefined when the argument is
// absent, null for `none` or for a function (which is not read), else the pattern.
function numberingArg(source, args) {
  const value = namedArg(source, args, "numbering");
  if (value === undefined) return undefined;
  return value?.name === "Str" ? innerOf(source, value) : null;
}

function styleArg(source, args) {
  const value = namedArg(source, args, "style");
  return value?.name === "Str" ? innerOf(source, value) : null;
}

function levelArg(source, args) {
  const value = namedArg(source, args, "level");
  if (value?.name !== "Int") return 1;
  return Math.min(6, Math.max(1, Number(source.slice(value.from, value.to))));
}

// The kind a call or raw node shows: an image, a table or raw text, else null.
function kindOfNode(source, node) {
  if (node?.name === "Raw") return "raw";
  if (node?.name !== "FuncCall") return null;
  switch (calleeName(source, node)) {
    case "image": return "image";
    case "table":
    case "grid": return "table";
    case "raw": return "raw";
    default: return null;
  }
}

// The kind shown by an argument: itself, or the first thing of a content block.
function kindOfArgument(source, node) {
  if (node.name !== "ContentBlock") return kindOfNode(source, node);
  let first = node.getChild("Markup")?.firstChild ?? null;
  while (first && TRIVIA.has(first.name)) first = first.nextSibling;
  if (first?.name === "Hash") return kindOfNode(source, first.nextSibling);
  return kindOfNode(source, first);
}

// The kind of a figure: a `kind:` argument wins, then the shape of its content
// (the first argument, then the first content block).
function figureKind(source, args) {
  const named = namedArg(source, args, "kind");
  if (named?.name === "Str") return KIND_NAMES.get(innerOf(source, named)) ?? innerOf(source, named);
  if (named?.name === "Ident") {
    const value = source.slice(named.from, named.to);
    return KIND_NAMES.get(value) ?? value;
  }
  const first = firstArgument(args);
  let kind = first && first.name !== "Named" ? kindOfArgument(source, first) : null;
  if (!kind) {
    const block = args.getChild("ContentBlock");
    kind = block ? kindOfArgument(source, block) : null;
  }
  return KIND_NAMES.get(kind) ?? "figure";
}

// The label node that follows `node` after spaces or tabs only, as in
// "#figure(...) <fig>", or null.
function labelNodeAfter(source, node) {
  let next = node.nextSibling;
  while (next?.name === "Space") next = next.nextSibling;
  return next?.name === "Label" && BLANK_GAP.test(source.slice(node.to, next.from)) ? next : null;
}

function labelAfter(source, node) {
  const label = labelNodeAfter(source, node);
  return label ? innerOf(source, label) : null;
}

// Whether only blanks follow `at` on its line.
function endsLine(source, at) {
  REST_OF_LINE.lastIndex = at;
  return REST_OF_LINE.test(source);
}

// The label at the end of a heading line: inside the heading's markup, or right
// after the heading node on the same line.
function headingLabel(source, heading) {
  let last = heading.lastChild;
  while (last) {
    while (last && TRIVIA.has(last.name)) last = last.prevSibling;
    if (last?.name !== "Markup") break;
    last = last.lastChild;
  }
  const label = last?.name === "Label" ? last : labelNodeAfter(source, heading);
  return label && endsLine(source, label.to) ? innerOf(source, label) : null;
}

// The elements of one file that the numbering and the citations read, in
// document order, and the names of the labels it defines. Labels of headings,
// figures and equations are bound here.
function scanTree(tree, source) {
  const events = [];
  const labelNames = [];
  tree.iterate({
    enter(ref) {
      const node = ref.node;
      switch (node.name) {
        case "Raw":
          return false;
        case "Equation": {
          const last = node.lastChild;
          const content = source.slice(node.from + 1, node.to - 1);
          // Display math: its content starts and ends with whitespace.
          if (last && last !== node.firstChild && last.name === "Dollar" && content.length > 1 && /^\s/.test(content) && /\s$/.test(content)) {
            events.push({ at: node.from, type: "equation", label: labelAfter(source, node) });
          }
          return false;
        }
        case "Label":
          // A label passed to a call is a reference, not a definition.
          if (node.parent?.name !== "Args") labelNames.push(innerOf(source, node));
          return;
        case "Heading": {
          const marker = node.firstChild;
          events.push({
            // The offset of the = marker, which is what the editor's token starts at.
            at: node.from,
            type: "heading",
            level: Math.min(Math.max(marker ? marker.to - marker.from : 1, 1), 6),
            numbering: undefined,
            label: headingLabel(source, node),
          });
          return;
        }
        case "Ref": {
          const marker = node.getChild("RefMarker");
          if (marker && !typstRefInWord(source, marker.from)) {
            events.push({ at: marker.from, type: "ref", key: source.slice(marker.from + 1, marker.to) });
          }
          return;
        }
        case "ModuleInclude": {
          const path = node.getChild("Str");
          if (path) events.push({ at: (hashBefore(node) ?? node).from, type: "include", path: innerOf(source, path) });
          return;
        }
        case "SetRule": {
          const hash = hashBefore(node);
          const args = node.getChild("Args");
          let target = node.firstChild?.nextSibling ?? null;
          while (target && TRIVIA.has(target.name)) target = target.nextSibling;
          const name = target ? source.slice(target.from, target.to) : "";
          if (!hash || !args || !SET_TARGETS.has(name)) return;
          events.push({
            at: hash.from,
            type: "set",
            target: name,
            numbering: numberingArg(source, args),
            style: styleArg(source, args),
          });
          return;
        }
        case "FuncCall": {
          const name = calleeName(source, node);
          const hash = hashBefore(node);
          const args = node.getChild("Args");
          if (!name || !hash || !args) return;
          if (name === "heading") {
            events.push({
              at: hash.from,
              type: "heading",
              level: levelArg(source, args),
              numbering: numberingArg(source, args),
              label: labelAfter(source, node),
            });
          } else if (name === "figure") {
            events.push({
              at: hash.from,
              type: "figure",
              kind: figureKind(source, args),
              numbering: numberingArg(source, args),
              label: labelAfter(source, node),
            });
          } else if (name === "bibliography") {
            events.push({ at: hash.from, type: "bibliography", style: styleArg(source, args) });
          } else if (name === "cite") {
            const first = firstArgument(args);
            if (first?.name === "Label") events.push({ at: hash.from, type: "cite", key: innerOf(source, first) });
          }
          return;
        }
        default:
      }
    },
  });
  return { events, labelNames };
}

function symbolCount(pattern) {
  return [...pattern].filter((ch) => SYMBOLS.includes(ch)).length;
}

function alpha(n) {
  return n >= 1 && n <= 26 ? String.fromCharCode(64 + n) : String(n);
}


function format(n, symbol) {
  switch (symbol) {
    case "a": return alpha(n).toLowerCase();
    case "A": return alpha(n);
    case "i": return roman(n).toLowerCase();
    case "I": return roman(n);
    default: return String(n);
  }
}

// Renders a numbering pattern with one value per counting symbol. The render
// stops at the first symbol without a value, and the punctuation at either end
// is dropped, so "1." gives "3" and "(1)" gives "4".
function render(pattern, values) {
  let out = "";
  let next = 0;
  for (const ch of pattern) {
    if (!SYMBOLS.includes(ch)) {
      out += ch;
      continue;
    }
    if (next >= values.length) break;
    out += format(values[next], ch);
    next += 1;
  }
  return out.replace(TRIM, "");
}

function bump(map, key) {
  map.set(key, (map.get(key) ?? 0) + 1);
}

/// Computes the facts of a Typst project, in the shape documentFacts returns.
/// `texts` maps each project path (no leading slash) to its source, `main` is
/// the main file, and `entries` the parsed bibliography entries. `open` is the
/// file shown in the editor: its numbered headings are listed in `headingNumbers`,
/// keyed by the offset of the = marker or of the #heading call.
///
/// Labels map to { number, kind }, where kind is 'section', 'equation' or a
/// figure kind ('figure', 'table', 'listing', or the name of a `kind:`). Only
/// labels bound to a numbered element are listed. A citation is a cite of a
/// bibliography key, or a @key that is no label of the project, and it takes
/// its number at its first use in reading order.
export function typstFacts(texts, main, open, entries = []) {
  const focus = open == null ? null : normalize(open);
  const headingNumbers = new Map();
  // The scan of each file. Without the parser there is no tree for any file,
  // and the project reads as empty.
  const scans = new Map();
  for (const path of Object.keys(texts)) {
    const source = texts[path] ?? "";
    const tree = typstTree(source, path);
    if (!tree) {
      scans.clear();
      break;
    }
    scans.set(path, scanTree(tree, source));
  }
  const labelNames = new Set();
  for (const { labelNames: names } of scans.values()) for (const name of names) labelNames.add(name);
  const entryKeys = new Set(rankEntries(entries).map((entry) => entry.key));

  const labels = new Map();
  const order = new Map();
  const cited = (key) => {
    if (!order.has(key)) order.set(key, order.size + 1);
  };
  // Heading counters by level. The per-kind counters count the whole document
  // (globals) and the current chapter (chapters, reset by each first-level heading).
  const counters = [0, 0, 0, 0, 0, 0];
  const globals = new Map();
  const chapters = new Map();
  const countValues = (pattern, key) =>
    symbolCount(pattern) >= 2 ? [counters[0], chapters.get(key)] : [globals.get(key)];
  let headingPattern = null;
  let figurePattern = "1";
  let equationPattern = null;
  let bibliographyStyle = null;
  let setStyle = null;

  const resolve = (name, from) => {
    const target = name.trim();
    const path = normalize(target.startsWith("/") ? target : folderOf(from) + target);
    return scans.has(path) ? path : null;
  };

  const scanFile = (path, stack) => {
    for (const event of scans.get(path).events) {
      switch (event.type) {
        case "heading": {
          const pattern = event.numbering !== undefined ? event.numbering : headingPattern;
          if (pattern === null) break;
          counters[event.level - 1] += 1;
          counters.fill(0, event.level);
          if (event.level === 1) chapters.clear();
          const values = symbolCount(pattern) >= 2 ? counters.slice(0, event.level) : [counters[event.level - 1]];
          const number = render(pattern, values);
          if (number && path === focus) headingNumbers.set(event.at, number);
          if (event.label && number) labels.set(event.label, { number, kind: "section" });
          break;
        }
        case "figure": {
          const pattern = event.numbering !== undefined ? event.numbering : figurePattern;
          if (pattern === null) break;
          bump(globals, event.kind);
          bump(chapters, event.kind);
          const number = render(pattern, countValues(pattern, event.kind));
          if (event.label && number) labels.set(event.label, { number, kind: event.kind });
          break;
        }
        case "equation": {
          if (equationPattern === null) break;
          bump(globals, "equation");
          bump(chapters, "equation");
          const number = render(equationPattern, countValues(equationPattern, "equation"));
          if (event.label && number) labels.set(event.label, { number, kind: "equation" });
          break;
        }
        case "set": {
          if (event.target === "cite") {
            setStyle = event.style ?? setStyle;
            break;
          }
          const value = event.numbering;
          if (value === undefined) break;
          if (event.target === "heading") headingPattern = value;
          else if (event.target === "figure") figurePattern = value;
          else equationPattern = value;
          break;
        }
        case "bibliography":
          bibliographyStyle = event.style ?? bibliographyStyle;
          break;
        case "cite":
          if (entryKeys.has(event.key)) cited(event.key);
          break;
        case "ref":
          if (!labelNames.has(event.key) && entryKeys.has(event.key)) cited(event.key);
          break;
        case "include": {
          const next = resolve(event.path, path);
          if (next && !stack.includes(next)) scanFile(next, [...stack, next]);
          break;
        }
      }
    }
  };

  const root = normalize(main);
  if (scans.has(root)) scanFile(root, [root]);

  const style = bibliographyStyle ?? setStyle;
  return {
    labels,
    citations: citationFacts(entries, order),
    acronyms: new Map(),
    textMacros: new Map(),
    mathMacros: {},
    theorems: new Map(),
    environmentNumbers: new Map(),
    headingNumbers,
    citeStyle: style && AUTHOR_YEAR.test(style) ? "author-year" : "numeric",
  };
}
