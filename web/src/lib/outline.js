import { latexTree, markdownTree } from "./source-trees.js";

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

function documentLines(source) {
  const lines = [];
  let start = 0;
  for (let i = 0; i <= source.length; i++) {
    if (i !== source.length && source[i] !== "\n" && source[i] !== "\r") continue;
    lines.push({ start, end: i, text: source.slice(start, i) });
    if (i === source.length) break;
    if (source[i] === "\r" && source[i + 1] === "\n") i++;
    start = i + 1;
  }
  return lines;
}

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

function addHeading(result, starts, title, level, from) {
  const clean = normalizeTitle(title);
  if (!clean || !Number.isInteger(level) || level < 1) return;
  result.push({ title: clean, level, from, line: lineFor(starts, from) });
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

function typstMask(source, lines) {
  const mask = new Uint8Array(source.length);
  // Typst block comments nest.  Line comments are marked to the end of their
  // line; a small quote state avoids treating // inside a string as a comment.
  let blockDepth = 0;
  let quote = false;
  let escaped = false;
  for (let i = 0; i < source.length; i++) {
    if (mask[i]) continue;
    const two = source.slice(i, i + 2);
    if (blockDepth) {
      markRange(mask, i, i + 1);
      if (two === "/*") { blockDepth++; markRange(mask, i, i + 2); i++; }
      else if (two === "*/") { blockDepth--; markRange(mask, i, i + 2); i++; }
      continue;
    }
    if (quote) {
      if (escaped) escaped = false;
      else if (source[i] === "\\") escaped = true;
      else if (source[i] === '"') quote = false;
      continue;
    }
    if (source[i] === '"') { quote = true; continue; }
    if (two === "/*") { blockDepth = 1; markRange(mask, i, i + 2); i++; continue; }
    if (two === "//") {
      let end = source.indexOf("\n", i);
      if (end < 0) end = source.length;
      markRange(mask, i, end);
      i = end - 1;
    }
  }

  // Raw fences inside comments must not open a raw block.  Do this after
  // comments have been masked, and count only visible fence markers.
  let raw = false;
  for (const line of lines) {
    let rawMarkers = 0;
    for (let at = line.start; at < line.end - 2; at++) {
      if (!mask[at] && source.slice(at, at + 3) === "```") { rawMarkers++; at += 2; }
    }
    if (raw) {
      markRange(mask, line.start, line.end);
      if (rawMarkers % 2 === 1) raw = false;
    } else if (rawMarkers) {
      markRange(mask, line.start, line.end);
      if (rawMarkers % 2 === 1) raw = true;
    }
  }
  return mask;
}

function balanced(source, mask, open, left, right) {
  let depth = 0;
  let quote = false;
  let escaped = false;
  for (let i = open; i < source.length; i++) {
    if (mask[i]) continue;
    const char = source[i];
    if (quote) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === '"') quote = false;
      continue;
    }
    if (char === '"') { quote = true; continue; }
    if (char === left) depth++;
    else if (char === right && --depth === 0) return i;
  }
  return -1;
}

function extractTypst(source, starts) {
  const lines = documentLines(source);
  const mask = typstMask(source, lines);
  const result = [];
  for (const line of lines) {
    if (mask[line.start]) continue;
    const text = visible(source, mask, line.start, line.end);
    const markup = /^(\s*)(={1,})(?:[ \t]+(.*)|[ \t]*)$/.exec(text);
    if (markup) addHeading(result, starts, markup[3] || "", markup[2].length, line.start + markup[1].length);
  }

  // Function-form headings are useful in generated Typst and are common in
  // templates: #heading(level: 2)[A title] and #heading[A title].
  for (let i = 0; i < source.length; i++) {
    if (mask[i] || source[i] !== "#" || !source.startsWith("#heading", i) || typstLiteralAt(source, i)) continue;
    const after = i + 8;
    if (/[A-Za-z0-9_-]/u.test(source[after] || "")) continue;
    let cursor = after;
    while (/\s/u.test(source[cursor] || "")) cursor++;
    let level = 1;
    if (source[cursor] === "(") {
      const close = balanced(source, mask, cursor, "(", ")");
      if (close < 0) continue;
      const options = visible(source, mask, cursor + 1, close);
      const match = /(?:^|,)\s*level\s*:\s*(\d+)/u.exec(options);
      if (match) level = Number(match[1]);
      cursor = close + 1;
      while (/\s/u.test(source[cursor] || "")) cursor++;
    }
    if (source[cursor] !== "[") continue;
    const close = balanced(source, mask, cursor, "[", "]");
    if (close < 0) continue;
    addHeading(result, starts, visible(source, mask, cursor + 1, close), level, i);
  }
  return result.sort((a, b) => a.from - b.from);
}

function typstLiteralAt(source, offset) {
  let lineStart = source.lastIndexOf("\n", offset - 1) + 1;
  let string = false;
  let raw = false;
  let escaped = false;
  for (let i = lineStart; i < offset; i++) {
    if (string) {
      if (escaped) escaped = false;
      else if (source[i] === "\\") escaped = true;
      else if (source[i] === '"') string = false;
      continue;
    }
    if (source[i] === '"') { string = true; continue; }
    if (source[i] === "`") {
      if (source.slice(i, i + 3) === "```") { i += 2; continue; }
      raw = !raw;
    }
  }
  return string || raw;
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
        // The underline is the last mark; the title is everything before it.
        from = node.from;
        to = marks.length ? marks[marks.length - 1].from : node.to;
      }
      // A trailing attribute list ({#id .class}) is not part of the title.
      if (last && last.name === "Attributes" && last.to <= to && !/\S/u.test(body.slice(last.to, to))) to = last.from;
      const lead = body.slice(node.from, node.to).length - body.slice(node.from, node.to).trimStart().length;
      addHeading(result, starts, body.slice(from, Math.max(from, to)), level, node.from + (match[1] === "Setext" ? lead : 0));
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
      if (ref.name !== "Command" || ref.from < skipUntil) return;
      const node = ref.node;
      const name = node.firstChild;
      if (!name || name.name !== "SectionName") return;
      const level = LATEX_LEVELS.get(source.slice(name.from, name.to).replace(/\*$/u, "").toLowerCase());
      if (!level) return false;
      // An unfinished [short title] means the command is still being typed.
      const optional = name.nextSibling;
      if (optional && optional.name === "OptionalArgument" && optional.lastChild?.name !== "OptClose") return false;
      let group = node.nextSibling;
      while (group && (group.name === "Comment" || (group.name === "Text" && !/\S/u.test(source.slice(group.from, group.to))))) group = group.nextSibling;
      if (!group || group.name !== "Group" || group.lastChild?.name !== "CloseBrace") return false;
      // The title is the braced source with comments and \verb spans dropped.
      let title = "";
      let cursor = group.from + 1;
      const end = group.lastChild.from;
      for (let child = group.firstChild; child; child = child.nextSibling) {
        if (child.name !== "Comment" && child.name !== "Verb" && child.name !== "VerbatimEnvironment") continue;
        title += source.slice(cursor, child.from);
        cursor = child.to;
      }
      title += source.slice(cursor, end);
      addHeading(result, starts, title, level, node.from);
      skipUntil = group.to;
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
    default:
      return [];
  }
}
