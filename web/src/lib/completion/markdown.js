// Completion for Markdown and Quarto sources: image paths, link targets and
// anchors, reference link ids and footnote ids. Quarto files are Markdown, so
// both formats share these sources.
import { dirname, normalizePath, pathOptions, resolvePath } from "./paths.js";

const MARKDOWN_PATH = /\.(?:md|markdown|qmd)$/i;
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "svg", "webp", "pdf"];
const PATH_CHARS = /^[^()\s]*$/;
const IMAGE = /!\[[^\]]*\]\(([^()\s]*)$/;
const LINK = /(?:^|[^!])\[[^\]]*\]\(([^()\s]*)$/;
const REFERENCE = /\[[^\]]*\]\[([^[\]\s]*)$/;
const FOOTNOTE = /\[\^([^[\]\s]*)$/;
const FOOTNOTE_START = /^ {0,3}\[\^/;
const REFERENCE_DEFINITION = /^ {0,3}\[(?!\^)([^\]]+)\]:/;
const FOOTNOTE_DEFINITION = /^ {0,3}\[\^([^\]\s]+)\]:/;
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;
const FRONT_MATTER_END = /^(?:---|\.\.\.)\s*$/;
const ATX = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/;

// Lines of a Markdown text, flagged when they are code. Fenced blocks and
// YAML front matter count as code, so a "# comment" there is not a heading.
function markdownLines(text) {
  const lines = String(text ?? "").split(/\r?\n/);
  const out = lines.map((line, index) => ({ text: line, number: index + 1, code: false }));
  let start = 0;
  if (lines[0] === "---") {
    const end = lines.findIndex((line, index) => index > 0 && FRONT_MATTER_END.test(line));
    if (end > 0) {
      for (let i = 0; i <= end; i++) out[i].code = true;
      start = end + 1;
    }
  }
  let fence = null;
  for (let i = start; i < lines.length; i++) {
    const open = lines[i].match(FENCE);
    if (fence) {
      out[i].code = true;
      if (open && open[1][0] === fence.marker && open[1].length >= fence.length && !open[2].trim()) fence = null;
    } else if (open) {
      out[i].code = true;
      fence = { marker: open[1][0], length: open[1].length };
    }
  }
  return out;
}

// GitHub-style slug as comrak produces it for header ids.
function slug(title) {
  return title.toLowerCase().replace(/[^\p{L}\p{N} _-]/gu, "").replace(/ /g, "-");
}

// ATX headings outside code, with ids in document order.
function headingsOf(text) {
  const used = new Set();
  const found = [];
  for (const line of markdownLines(text)) {
    if (line.code) continue;
    const match = line.text.match(ATX);
    if (!match) continue;
    // Closing hashes go first, so "{#id}" is found at the very end.
    let title = (match[2] ?? "").replace(/(?:^|[ \t]+)#+[ \t]*$/, "");
    const explicit = title.match(/[ \t]*\{#([^\s{}]+)\}[ \t]*$/);
    let id;
    if (explicit) {
      id = explicit[1];
      title = title.slice(0, explicit.index);
    }
    title = title.replace(/[ \t]*\{[^{}]*\}[ \t]*$/, "").trim();
    if (!explicit) {
      const base = slug(title);
      if (!base) continue;
      id = base;
      for (let n = 1; used.has(id); n++) id = `${base}-${n}`;
    }
    if (!id) continue;
    used.add(id);
    found.push({ id, title, line: line.number });
  }
  return found;
}

function anchorOptions(text, prefix) {
  return headingsOf(text).map((heading) => ({ label: prefix + heading.id, type: "heading", detail: heading.title }));
}

// Ids declared by definition lines matching `pattern` (group 1 is the id).
function definitionIds(text, pattern) {
  const ids = new Set();
  for (const line of markdownLines(text)) {
    if (line.code) continue;
    const match = line.text.match(pattern);
    if (match) ids.add(match[1]);
  }
  return [...ids].map((id) => ({ label: id, type: "variable", detail: "" }));
}

// Same idea as markdownCode in bibliography.js: inside a fence or an unclosed
// inline code span before the cursor.
function markdownCode(text, position) {
  const before = text.slice(0, position);
  let fence = null;
  for (const line of before.split("\n")) {
    const match = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (!match) continue;
    if (!fence) fence = { marker: match[1][0], length: match[1].length };
    else if (match[1][0] === fence.marker && match[1].length >= fence.length && !match[2].trim()) fence = null;
  }
  if (fence) return true;
  const line = before.slice(before.lastIndexOf("\n") + 1);
  let ticks = 0;
  for (const match of line.matchAll(/`+/g)) {
    const prefix = line.slice(0, match.index);
    if ((prefix.match(/\\+$/)?.[0].length || 0) % 2) continue;
    if (!ticks) ticks = match[0].length;
    else if (ticks === match[0].length) ticks = 0;
  }
  return ticks > 0;
}

function result(from, validFor, options) {
  return options.length ? { from, validFor, options } : null;
}

/** Labels for cross references: one per ATX heading in every Markdown text. */
export function markdownLabels(texts = {}) {
  const labels = [];
  for (const path of Object.keys(texts).sort()) {
    if (!MARKDOWN_PATH.test(path)) continue;
    for (const heading of headingsOf(texts[path])) {
      labels.push({ id: heading.id, kind: "heading", path, line: heading.line });
    }
  }
  return labels;
}

/** Completion source for Markdown and Quarto. Returns null outside those formats. */
export function markdownCompletion(project) {
  return (context) => {
    const current = project();
    if (current.format !== "markdown" && current.format !== "quarto") return null;
    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, context.pos - line.from);
    let kind = null, match = null;
    if ((match = before.match(IMAGE))) kind = "image";
    else if ((match = before.match(LINK))) kind = "link";
    else if ((match = before.match(REFERENCE))) kind = "reference";
    else if ((match = before.match(FOOTNOTE))) kind = "footnote";
    if (!kind) return null;
    // A footnote at the start of a line is a definition being typed, not a reference.
    if (kind === "footnote" && FOOTNOTE_START.test(before)) return null;

    const text = context.state.doc.toString();
    if (markdownCode(text, context.pos)) return null;
    const partial = match[1] ?? "";
    const from = context.pos - partial.length;
    const directory = dirname(current.path);

    if (kind === "image") {
      return result(from, PATH_CHARS, pathOptions(current.files, directory, {
        extensions: IMAGE_EXTENSIONS, exclude: current.path,
      }));
    }

    if (kind === "reference") {
      return result(from, /^[^[\]\s]*$/, definitionIds(text, REFERENCE_DEFINITION));
    }

    if (kind === "footnote") {
      return result(from, /^[^[\]\s]*$/, definitionIds(text, FOOTNOTE_DEFINITION));
    }

    // Link target. "#frag" alone names this file; "other.md#frag" names another.
    const hash = partial.indexOf("#");
    if (hash < 0) {
      const files = pathOptions(current.files, directory);
      return result(from, PATH_CHARS, [...files, ...anchorOptions(text, "#")]);
    }
    const file = partial.slice(0, hash);
    if (!file) return result(from, PATH_CHARS, anchorOptions(text, "#"));
    const target = resolvePath(directory, file);
    if (!MARKDOWN_PATH.test(target)) return null;
    let source = text;
    if (target !== normalizePath(current.path)) {
      const texts = current.texts;
      if (!Object.hasOwn(texts, target)) return null;
      source = texts[target];
    }
    return result(from, PATH_CHARS, anchorOptions(source, `${file}#`));
  };
}
