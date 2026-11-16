import { latexTree, markdownTree, orgTree, typstTree } from "./source-trees.js";

/*
 * Extract a small, source-oriented outline without compiling or executing the
 * document.  All offsets deliberately use JavaScript string indices: these
 * are UTF-16 offsets, which is also what CodeMirror uses for documents.
 */

const LATEX_LEVELS = new Map([
  ["part", 1],
  ["chapter", 2],
  ["section", 3],
  ["subsection", 4],
  ["subsubsection", 5],
  ["paragraph", 6],
  ["subparagraph", 7],
]);

function lineStarts(source) {
  const starts = [0];
  for (let i = 0; i < source.length; i++) {
    if (source[i] === "\r") {
      if (source[i + 1] === "\n") i++;
      starts.push(i + 1);
    } else if (source[i] === "\n") {
      starts.push(i + 1);
    }
  }
  return starts;
}

function lineFor(starts, offset) {
  let lo = 0;
  let hi = starts.length;
  while (lo + 1 < hi) {
    const middle = (lo + hi) >> 1;
    if (starts[middle] <= offset) lo = middle;
    else hi = middle;
  }
  return lo + 1;
}

function normalizeTitle(value) {
  return value.replace(/\s+/gu, " ").trim();
}

// `extra` holds format-specific fields (Org TODO state, priority, tags); it is
// spread in only when given, so other formats keep their exact shape.
function addHeading(result, starts, title, level, from, extra) {
  const clean = normalizeTitle(title);
  if (!clean || !Number.isInteger(level) || level < 1) return;
  result.push({ title: clean, level, from, line: lineFor(starts, from), ...extra });
}

function markRange(mask, start, end) {
  const from = Math.max(0, start);
  const to = Math.min(mask.length, Math.max(from, end));
  mask.fill(1, from, to);
}

function visible(source, mask, from, to) {
  let value = "";
  for (let i = from; i < to; i++) if (!mask[i]) value += source[i];
  return value;
}

// The visible text of a Typst node range, with comments dropped.
function typstText(tree, source, from, to) {
  let value = "";
  let cursor = from;
  tree.iterate({
    from,
    to,
    enter(ref) {
      if (ref.name !== "LineComment" && ref.name !== "BlockComment") return;
      const start = Math.max(cursor, ref.from);
      const end = Math.min(to, ref.to);
      if (end <= start) return;
      value += source.slice(cursor, start);
      cursor = end;
    },
  });
  return value + source.slice(cursor, to);
}

function extractTypst(source, starts) {
  const tree = typstTree(source);
  if (!tree) return [];
  const result = [];
  tree.iterate({
    enter(ref) {
      if (ref.name === "Heading") {
        // The level is the length of the "=" marker. The title is the markup
        // of the heading, so a trailing label or comment is not part of it.
        const marker = ref.node.getChild("HeadingMarker");
        const markup = ref.node.getChild("Markup");
        const level = marker ? marker.to - marker.from : 0;
        addHeading(result, starts, markup ? typstText(tree, source, markup.from, markup.to) : "", level, ref.from);
        return;
      }
      if (ref.name !== "FuncCall") return;
      // Function-form headings are useful in generated Typst and are common in
      // templates: #heading(level: 2)[A title] and #heading[A title].
      const callee = ref.node.firstChild;
      if (!callee || callee.name !== "Ident" || source.slice(callee.from, callee.to) !== "heading") return;
      const args = ref.node.getChild("Args");
      const content = args?.getChild("ContentBlock");
      if (!content) return;
      let level = 1;
      for (let arg = args.firstChild; arg; arg = arg.nextSibling) {
        if (arg.name !== "Named") continue;
        const name = arg.firstChild;
        const value = arg.lastChild;
        if (name?.name === "Ident" && source.slice(name.from, name.to) === "level" && value?.name === "Int") {
          level = Number(source.slice(value.from, value.to));
          break;
        }
      }
      const hash = ref.node.prevSibling;
      const from = hash?.name === "Hash" ? hash.from : ref.from;
      const markup = content.getChild("Markup");
      addHeading(result, starts, markup ? typstText(tree, source, markup.from, markup.to) : "", level, from);
    },
  });
  return result.sort((a, b) => a.from - b.from);
}

function htmlTagEnd(source, start) {
  let quote = "";
  for (let i = start; i < source.length; i++) {
    if (quote) { if (source[i] === quote) quote = ""; }
    else if (source[i] === '"' || source[i] === "'") quote = source[i];
    else if (source[i] === ">") return i;
  }
  return -1;
}

function htmlCommentAndRawMask(source) {
  const mask = new Uint8Array(source.length);
  for (let i = 0; i < source.length;) {
    if (source.startsWith("<!--", i)) {
      const close = source.indexOf("-->", i + 4);
      markRange(mask, i, close < 0 ? source.length : close + 3);
      i = close < 0 ? source.length : close + 3;
      continue;
    }
    if (source[i] !== "<") { i++; continue; }
    const end = htmlTagEnd(source, i + 1);
    if (end < 0) break;
    const tag = source.slice(i, end + 1);
    const raw = /^<\s*(script|style|pre|textarea)\b/i.exec(tag);
    if (!raw || /\/\s*>$/u.test(tag)) { i = end + 1; continue; }
    const close = new RegExp(`<\\/\\s*${raw[1]}\\s*>`, "ig");
    close.lastIndex = end + 1;
    const match = close.exec(source);
    const to = match ? match.index + match[0].length : source.length;
    markRange(mask, i, to);
    i = to;
  }
  return mask;
}

function htmlText(source, mask, from, to) {
  const fragment = visible(source, mask, from, to);
  let text = "";
  for (let i = 0; i < fragment.length; i++) {
    if (fragment[i] === "<" && /^<\/?[a-z]/i.test(fragment.slice(i, i + 3))) {
      const end = htmlTagEnd(fragment, i + 1);
      if (end >= 0) {
        if (/^<br\b/i.test(fragment.slice(i, end + 1))) text += " ";
        i = end;
        continue;
      }
    }
    text += fragment[i];
  }
  text = text.replace(/&(#x[\da-f]+|#\d+|amp|lt|gt|quot|apos|nbsp);/giu, (_, entity) => {
    const lower = entity.toLowerCase();
    if (lower === "amp") return "&";
    if (lower === "lt") return "<";
    if (lower === "gt") return ">";
    if (lower === "quot") return '"';
    if (lower === "apos") return "'";
    if (lower === "nbsp") return " ";
    if (lower.startsWith("#x") || lower.startsWith("#")) {
      const code = Number.parseInt(lower.startsWith("#x") ? lower.slice(2) : lower.slice(1), lower.startsWith("#x") ? 16 : 10);
      if (Number.isInteger(code) && code >= 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff)) return String.fromCodePoint(code);
    }
    return _;
  });
  return text;
}

function extractHtml(source, starts) {
  const mask = htmlCommentAndRawMask(source);
  const result = [];
  for (let i = 0; i < source.length;) {
    if (mask[i] || source[i] !== "<") { i++; continue; }
    const end = htmlTagEnd(source, i + 1);
    if (end < 0) break;
    const tag = source.slice(i, end + 1);
    const opening = tag.match(/^<\s*h([1-6])\b/i);
    if (!opening || /\/\s*>$/u.test(tag)) { i = end + 1; continue; }
    const level = Number(opening[1]);
    const close = new RegExp(`<\\/\\s*h${level}\\s*>`, "ig");
    close.lastIndex = end + 1;
    let closing;
    while ((closing = close.exec(source))) {
      if (!mask[closing.index]) break;
    }
    if (closing) addHeading(result, starts, htmlText(source, mask, end + 1, closing.index), level, i);
    i = end + 1;
  }
  return result;
}

function extractMarkdown(source, starts) {
  const { tree, body } = markdownTree(source);
  const result = [];
  tree.iterate({
    enter(ref) {
      const match = /^(ATX|Setext)Heading([1-6])$/.exec(ref.name);
      if (!match) return;
      const node = ref.node;
      const level = Number(match[2]);
      const marks = [];
      let last = null;
      for (let child = node.firstChild; child; child = child.nextSibling) {
        if (child.name === "HeaderMark") marks.push(child);
        else last = child;
      }
      let from;
      let to;
      if (match[1] === "ATX") {
        // The opening mark is first; a closing sequence is a second mark.
        from = marks.length ? marks[0].to : node.from;
        to = marks.length > 1 ? marks[marks.length - 1].from : node.to;
      } else {
        // The underline is the last mark. A paragraph line above the title
        // belongs to the heading in CommonMark, but the outline has always
        // shown only the line just above the underline, as Pandoc reads it.
        to = marks.length ? marks[marks.length - 1].from : node.to;
        const text = body.slice(node.from, to).replace(/\s+$/u, "");
        from = node.from + text.lastIndexOf("\n") + 1;
      }
      // A trailing attribute list ({#id .class}) is not part of the title.
      if (last && last.name === "Attributes" && last.to <= to && !/\S/u.test(body.slice(last.to, to))) to = last.from;
      const start = match[1] === "Setext" ? from + (body.slice(from, to).length - body.slice(from, to).trimStart().length) : node.from;
      addHeading(result, starts, body.slice(from, Math.max(from, to)), level, start);
      return false;
    },
  });
  return result;
}

function extractLatex(source, starts) {
  const tree = latexTree(source);
  const result = [];
  let skipUntil = 0;
  tree.iterate({
    enter(ref) {
      if (ref.name !== "SectionName" || ref.from < skipUntil) return;
      // Normally the first child of a Command; inside unclosed math it is a bare item.
      const name = ref.node;
      const node = name.parent?.name === "Command" ? name.parent : name;
      const level = LATEX_LEVELS.get(source.slice(name.from + 1, name.to).replace(/\*$/u, "").toLowerCase());
      if (!level) return false;
      // An unfinished [short title] means the command is still being typed.
      const optional = name.nextSibling;
      if (optional && optional.name === "OptionalArgument" && optional.lastChild?.name !== "OptClose") return false;
      let group = node.nextSibling;
      while (group && (group.name === "Comment" || (group.name === "Text" && !/\S/u.test(source.slice(group.from, group.to))))) group = group.nextSibling;
      if (!group || (group.name !== "Group" && group.name !== "MathGroup") || group.lastChild?.name !== "CloseBrace") return false;
      // The title is the braced source with comments and \verb spans dropped.
      let title = "";
      let cursor = group.from + 1;
      const end = group.lastChild.from;
      // At any depth: a comment inside a nested group is dropped too.
      group.cursor().iterate((child) => {
        if (child.name !== "Comment" && child.name !== "Verb" && child.name !== "VerbatimEnvironment") return;
        title += source.slice(cursor, child.from);
        cursor = child.to;
        return false;
      });
      title += source.slice(cursor, end);
      addHeading(result, starts, title, level, node.from);
      skipUntil = group.to;
      return false;
    },
  });
  return result;
}

// The keywords whose #+TODO: lines declare TODO states (as in the Org tree).
const ORG_TODO_KEYS = new Set(["TODO", "SEQ_TODO", "TYP_TODO"]);

// The states that end a sequence: DONE, and the words after `|` in each TODO
// keyword line (the last word when a line has no bar). Same rule as the
// visual Org markup, which colours these states.
function orgDoneStates(tree, source) {
  const done = new Set(["DONE"]);
  for (let child = tree.topNode.firstChild; child; child = child.nextSibling) {
    if (child.name !== "Keyword") continue;
    const key = child.getChild("KeywordKey");
    if (!key || !ORG_TODO_KEYS.has(source.slice(key.from, key.to).replace(/\[.*$/s, "").toUpperCase())) continue;
    const value = child.getChild("KeywordValue");
    const words = value ? source.slice(value.from, value.to).split(/\s+/).filter(Boolean) : [];
    const bar = words.indexOf("|");
    const ending = bar < 0 ? words.slice(-1) : words.slice(bar + 1);
    for (const word of ending) done.add(word.replace(/\(.*\)$/, ""));
  }
  return done;
}

// The optional fields of one Org headline. Each is present only when the
// headline has it: `todo` and `done` for a TODO keyword, `priority` for a
// cookie like [#A], `tags` for a non-empty tag list.
function orgHeadlineFields(node, source, done) {
  const fields = {};
  const todo = node.getChild("TodoKeyword");
  if (todo) {
    fields.todo = source.slice(todo.from, todo.to);
    fields.done = done.has(fields.todo);
  }
  const priority = node.getChild("Priority");
  if (priority) fields.priority = source.slice(priority.from, priority.to).slice(2, -1);
  const tags = [];
  for (let tag = node.getChild("Tags")?.firstChild; tag; tag = tag.nextSibling) {
    if (tag.name === "Tag") tags.push(source.slice(tag.from, tag.to));
  }
  if (tags.length) fields.tags = tags;
  return fields;
}

// Headlines, as the Org tree reads them: the title is the Title node, without
// the TODO keyword, priority cookie and tags, which are reported as their own
// fields. A COMMENT headline is not listed.
function extractOrg(source, starts) {
  const tree = orgTree(source);
  if (!tree) return [];
  const done = orgDoneStates(tree, source);
  const result = [];
  tree.iterate({
    enter(ref) {
      if (ref.name === "Section" || ref.name === "Document") return;
      if (ref.name !== "Headline") return false;
      const stars = ref.node.getChild("Stars");
      const title = ref.node.getChild("Title");
      const text = title ? source.slice(title.from, title.to) : "";
      if (stars && !/^COMMENT(?:[ \t]|$)/.test(text)) {
        addHeading(result, starts, text, stars.to - stars.from, ref.from, orgHeadlineFields(ref.node, source, done));
      }
      return false;
    },
  });
  return result;
}

/**
 * Return headings in source order for one of the supported source formats.
 * Unsupported formats intentionally return an empty outline.
 */
export function extractOutline(source, format) {
  if (typeof source !== "string") return [];
  const starts = lineStarts(source);
  switch (String(format || "").toLowerCase()) {
    case "markdown":
    case "md":
    case "quarto":
    case "qmd":
      return extractMarkdown(source, starts);
    case "typst":
    case "typ":
      return extractTypst(source, starts);
    case "html":
    case "htm":
      return extractHtml(source, starts);
    case "latex":
    case "tex":
      return extractLatex(source, starts);
    case "org":
      return extractOrg(source, starts);
    default:
      return [];
  }
}
