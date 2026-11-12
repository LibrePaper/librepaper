// Turns the HTML on the clipboard (Word, Google Docs, LibreOffice, web pages)
// into source text for the open file's format. The conversion is pure: it
// reads a neutral tree and never touches the DOM, so Node tests can run it
// with a hand-built tree. Only parseClipboardHtml reads a DOMParser, and that
// one is injectable. The editor pastes the plain text whenever this returns
// null, so a paste from a plain-text source changes nothing.

import { markupFamily } from "./editing-shortcuts.js";

/** @typedef {{ tag: string, attrs: Record<string, string>, children: Array<Element | string> }} Element */
/** @typedef {{ kind: "text", s: string, b: boolean, i: boolean, c: boolean, m?: boolean } | { kind: "link", href: string, inl: Inline[] } | { kind: "br" }} Inline */
/** @typedef {"latex" | "typst" | "markdown" | "org"} Family */

/// Dropped with their content: none of it is prose a reader should see as source.
const SKIP = new Set(["script", "style", "svg", "button", "input", "select", "textarea", "img", "template", "noscript", "head", "title", "meta", "link", "o:p", "iframe", "video", "audio", "canvas", "object"]);
/// Block containers with no markup of their own: their children are converted as blocks.
const CONTAINERS = new Set(["html", "body", "div", "section", "article", "header", "footer", "main", "nav", "aside", "address", "figure", "figcaption", "center", "details", "summary", "form", "fieldset", "dl", "dt", "dd"]);
const BLOCKS = new Set([...CONTAINERS, "p", "li", "ul", "ol", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "pre", "hr"]);
const HEADING = /^h[1-6]$/;
const CODE_TAGS = new Set(["code", "kbd", "samp", "tt"]);
/// Only web and mail links become links. Fragments and javascript: keep their text.
const LINK_HREF = /^(https?:|mailto:)/i;
/// A Word list marker: a bullet, or a number, letter or roman numeral with "." or ")".
const BULLET_OR_NUMBER = /^(?:[^\p{L}\p{N}]|o|\(?(?:\d+|[a-zA-Z]|[ivxIVX]+)[.)])$/u;
const ORDERED_MARKER = /^\(?(?:\d+|[a-zA-Z]|[ivxIVX]+)[.)]$/;
const LATEX_SECTIONS = ["section", "subsection", "subsubsection", "paragraph", "subparagraph", "subparagraph"];
const BR = { latex: "\\\\\n", typst: "\\\n", markdown: "\\\n", org: "\\\\\n" };
const LATEX_SPECIAL = { "\\": "\\textbackslash{}", "{": "\\{", "}": "\\}", $: "\\$", "&": "\\&", "%": "\\%", "#": "\\#", _: "\\_", "~": "\\textasciitilde{}", "^": "\\textasciicircum{}" };

/* ------------------------------------------------------- the tree from a DOM */

/// Copies a DOM node into the neutral tree. Comments and the head-like nodes are dropped here.
/** @param {any} node @returns {Element | string | null} */
export function domToTree(node) {
  if (node.nodeType === 3) return node.nodeValue;
  if (node.nodeType !== 1) return null;
  const tag = node.nodeName.toLowerCase();
  if (["script", "style", "template", "noscript", "head", "meta", "title", "link"].includes(tag)) return null;
  /** @type {Record<string, string>} */
  const attrs = {};
  for (const attr of Array.from(node.attributes ?? [])) attrs[attr.name.toLowerCase()] = attr.value;
  const children = [];
  for (const child of Array.from(node.childNodes ?? [])) {
    const converted = domToTree(child);
    if (converted !== null) children.push(converted);
  }
  return { tag, attrs, children };
}

/// Parses clipboard HTML in a detached document. Nothing is inserted into the page.
/** @returns {Element} */
export function parseClipboardHtml(html, DOMParserImpl = globalThis.DOMParser) {
  return /** @type {Element} */ (domToTree(new DOMParserImpl().parseFromString(html, "text/html").body));
}

/* ----------------------------------------------------- style and inline text */

/// The value of one CSS property, lowercased, or undefined. Anchored so that
/// `mso-bidi-font-weight` does not answer for `font-weight`.
const styleValue = (style, name) => {
  const match = new RegExp(`(?:^|;)\\s*${name}\\s*:\\s*([^;]+)`, "i").exec(style);
  return match ? match[1].trim().toLowerCase() : undefined;
};

/// Font families that mark code. Word and Google Docs mark code only by font.
const MONO_FAMILIES = new Set(["consolas", "courier new", "courier", "menlo", "monaco", "lucida console", "source code pro", "roboto mono", "fira code", "fira mono", "jetbrains mono", "dejavu sans mono", "liberation mono", "inconsolata", "cascadia code", "cascadia mono", "sf mono", "ibm plex mono", "ubuntu mono", "noto sans mono", "andale mono", "monospace"]);

/// Whether a font-family value starts with a monospace family. Only the first family counts.
const isMonoFamily = (value) => MONO_FAMILIES.has(value.split(",")[0].trim().replace(/^["']|["']$/g, "").trim());

/// The emphasis an element gives its content. Its style wins over its tag, so
/// Google Docs' `<b style="font-weight:normal">` wrapper is not bold.
function applyStyle(el, ctx) {
  const style = el.attrs.style ?? "";
  const weight = styleValue(style, "font-weight");
  const fontStyle = styleValue(style, "font-style");
  const fontFamily = styleValue(style, "font-family");
  let b = ctx.b;
  if (weight !== undefined) b = /^(bold|bolder|[6-9]00)$/.test(weight);
  else if (el.tag === "b" || el.tag === "strong") b = true;
  if (ctx.noBold) b = false;
  let i = ctx.i;
  if (fontStyle !== undefined) i = /^(italic|oblique)/.test(fontStyle);
  else if (el.tag === "i" || el.tag === "em" || el.tag === "cite") i = true;
  const mono = fontFamily !== undefined && isMonoFamily(fontFamily);
  // `m` remembers code marked only by font, which is how word processors write
  // code blocks. A later non-monospace font ends it, as the style wins for bold:
  // Google Docs puts Courier New on a <li> for its "o" bullet and Aptos on the text.
  let c = ctx.c;
  let m = Boolean(ctx.m);
  if (mono) [c, m] = [true, true];
  else if (fontFamily !== undefined && m) [c, m] = [false, false];
  if (CODE_TAGS.has(el.tag)) c = true;
  return { b, i, c, m, noBold: ctx.noBold };
}

const plainText = (s) => ({ kind: "text", s, b: false, i: false, c: false });

/// The text of inline tokens, with no markup. Used to compare a link's text with its URL.
const tokensText = (inl) => inl.map((t) => (t.kind === "text" ? t.s.replace(/\u00a0/g, " ") : t.kind === "br" ? " " : tokensText(t.inl))).join("");

/// Raw text of a node, with line breaks where <br> is. Used for list markers and <pre>.
const textRaw = (node) => {
  if (typeof node === "string") return node;
  if (node.tag === "br") return "\n";
  if (SKIP.has(node.tag)) return "";
  return node.children.map(textRaw).join("");
};

/// Whether a node holds any block-level element, which decides whether it can stay inline.
const containsBlock = (node) => typeof node !== "string" && (BLOCKS.has(node.tag) || node.children.some(containsBlock));

/// The inline tokens of a node, for a context of emphasis. Block elements inside
/// inline content become spaces, so their text does not run into the next word.
/** @returns {Inline[]} */
function inlinesOf(node, ctx, st) {
  if (typeof node === "string") {
    if ((ctx.b || ctx.i || ctx.c) && node.trim()) st.found = true;
    return [{ kind: "text", s: node, b: ctx.b, i: ctx.i, c: ctx.c, m: Boolean(ctx.m) }];
  }
  if (SKIP.has(node.tag)) return [];
  if (node.tag === "br") return [{ kind: "br" }];
  const inner = applyStyle(node, ctx);
  if (node.tag === "a") {
    const href = (node.attrs.href ?? "").trim();
    const content = node.children.flatMap((child) => inlinesOf(child, inner, st));
    if (!LINK_HREF.test(href) || !tokensText(content).trim()) return content;
    st.found = true;
    return [{ kind: "link", href, inl: content }];
  }
  if (BLOCKS.has(node.tag)) {
    return [plainText(" "), ...node.children.flatMap((child) => inlinesOf(child, ctx, st)), plainText(" ")];
  }
  return node.children.flatMap((child) => inlinesOf(child, inner, st));
}

/// Collapses whitespace, merges adjacent runs with the same emphasis, and drops
/// empty markup. Edge spaces of an emphasis run are moved out by `normalize`.
/** @returns {Inline[]} */
function collapse(inl) {
  /** @type {Inline[]} */
  const out = [];
  const endsSpace = () => {
    const last = out[out.length - 1];
    return !last || last.kind === "br" || (last.kind === "text" && /\s$/.test(last.s));
  };
  const add = (s, b, i, c) => {
    const last = out[out.length - 1];
    if (last?.kind === "text" && last.b === b && last.i === i && last.c === c) last.s += s;
    else out.push({ kind: "text", s, b, i, c });
  };
  const plain = (s) => {
    if (s.startsWith(" ") && endsSpace()) s = s.slice(1);
    if (s) add(s, false, false, false);
  };
  const trimSpaces = () => {
    const last = out[out.length - 1];
    if (last?.kind === "text") {
      last.s = last.s.replace(/ +$/, "");
      if (!last.s) out.pop();
    }
  };
  for (const tok of inl) {
    if (tok.kind === "br") {
      if (!out.length) continue;
      trimSpaces();
      out.push({ kind: "br" });
    } else if (tok.kind === "link") {
      const raw = tokensText(tok.inl);
      if (/^\s/.test(raw)) plain(" ");
      const content = normalize(tok.inl);
      if (!content.length) continue;
      out.push({ kind: "link", href: tok.href, inl: content });
      if (/\s$/.test(raw)) plain(" ");
    } else {
      // A flagged run keeps its inner spaces so that runs split across spans merge.
      const s = tok.s.replace(/\s+/g, " ");
      if (!(tok.b || tok.i || tok.c)) plain(s);
      else if (!s.trim()) plain(" ");
      else add(s.startsWith(" ") && endsSpace() ? s.slice(1) : s, tok.b, tok.i, tok.c);
    }
  }
  while (out[out.length - 1]?.kind === "br") out.pop();
  trimSpaces();
  return out;
}

/// Moves the spaces an emphasis run starts or ends with outside its markup, so
/// Markdown, Typst and Org emphasis hugs the text, then collapses once more.
/** @returns {Inline[]} */
function normalize(inl) {
  const hugged = [];
  for (const tok of collapse(inl)) {
    if (tok.kind !== "text" || !(tok.b || tok.i || tok.c) || !/^ | $/.test(tok.s)) {
      hugged.push(tok);
      continue;
    }
    const core = tok.s.trim();
    if (tok.s.startsWith(" ")) hugged.push(plainText(" "));
    if (core) hugged.push({ ...tok, s: core });
    if (tok.s.endsWith(" ") && core) hugged.push(plainText(" "));
  }
  return collapse(hugged);
}

/* ------------------------------------------------------------- the blocks */

/// A Word list paragraph's level and kind. Its marker span (a bullet or a
/// number in a span before the text) is dropped; the browser keeps that span
/// because it keeps the `<![if !supportLists]>` markers only as comments.
function listEntry(p, ctx, st) {
  const level = Number(/mso-list:\s*l\d+\s+level(\d+)/i.exec(p.attrs.style ?? "")?.[1] ?? 1) - 1;
  const kids = p.children.slice();
  const first = kids.findIndex((kid) => typeof kid !== "string");
  let ordered = false;
  if (first >= 0) {
    const mark = textRaw(kids[first]).replace(/\s+/g, "");
    if (mark && BULLET_OR_NUMBER.test(mark)) {
      ordered = ORDERED_MARKER.test(mark);
      kids.splice(first, 1);
    }
  }
  return { level, ordered, inl: normalize(kids.flatMap((kid) => inlinesOf(kid, ctx, st))) };
}

/// Flat list entries from one <ul> or <ol>, nested by aria-level when present,
/// otherwise by structure: a list inside an item, or a list directly inside a list (Google Docs).
function flattenList(el, depth, ctx, st) {
  const entries = [];
  const ordered = el.tag === "ol";
  let index = 0;
  for (const child of el.children) {
    if (typeof child === "string" || SKIP.has(child.tag)) continue;
    if (child.tag === "ul" || child.tag === "ol") {
      entries.push(...flattenList(child, depth + 1, ctx, st));
      continue;
    }
    const aria = Number(child.attrs["aria-level"]);
    const level = Number.isInteger(aria) && aria >= 1 ? aria - 1 : depth;
    const itemCtx = applyStyle(child, ctx);
    const own = child.children.filter((kid) => typeof kid === "string" || (kid.tag !== "ul" && kid.tag !== "ol"));
    const nested = child.children.filter((kid) => typeof kid !== "string" && (kid.tag === "ul" || kid.tag === "ol"));
    const inl = normalize(own.flatMap((kid) => inlinesOf(kid, itemCtx, st)));
    const start = ordered && index === 0 ? Number.parseInt(el.attrs.start, 10) : Number.NaN;
    index += 1;
    if (inl.length) entries.push({ level, ordered, inl, start: Number.isFinite(start) ? start : undefined });
    for (const list of nested) entries.push(...flattenList(list, level + 1, itemCtx, st));
  }
  return entries;
}

/// Nests flat entries into list blocks: an entry belongs under the last entry
/// with a lower level, and a new list starts when the kind (bullet or number) changes.
function nestEntries(entries) {
  const roots = [];
  const stack = [];
  for (const entry of entries) {
    while (stack.length && stack[stack.length - 1].level >= entry.level) stack.pop();
    const holder = stack.length ? stack[stack.length - 1].item.children : roots;
    let list = holder[holder.length - 1];
    if (!list || list.ordered !== entry.ordered) {
      list = { type: "list", ordered: entry.ordered, start: entry.start, items: [] };
      holder.push(list);
    }
    const item = { inl: entry.inl, children: [] };
    list.items.push(item);
    stack.push({ level: entry.level, item });
  }
  return roots;
}

/// One table cell as inline tokens: its blocks joined with spaces.
function cellInline(td, ctx, st) {
  const parts = [];
  const visit = (block) => {
    if (block.type === "para" || block.type === "heading") parts.push(block.inl);
    else if (block.type === "list") {
      for (const item of block.items) {
        parts.push(item.inl);
        item.children.forEach(visit);
      }
    } else if (block.type === "quote") block.blocks.forEach(visit);
    // A cell written in a monospace font keeps its code markup, on one line.
    else if (block.type === "pre") parts.push([{ kind: "text", s: block.text.replace(/\n/g, " "), b: false, i: false, c: true }]);
  };
  blocksFrom(td.children, applyStyle(td, ctx), st).forEach(visit);
  const joined = [];
  parts.forEach((part, index) => {
    if (index) joined.push(plainText(" "));
    joined.push(...part.map((tok) => (tok.kind === "br" ? plainText(" ") : tok)));
  });
  return normalize(joined);
}

/// A table block: rows of cells, each cell once (colspan and rowspan are ignored).
function tableBlock(el, ctx, st) {
  const rows = [];
  const walk = (node, head) => {
    for (const child of node.children) {
      if (typeof child === "string") continue;
      if (child.tag === "tr") rows.push({ tr: child, head });
      else if (child.tag === "thead") walk(child, true);
      else if (child.tag === "tbody" || child.tag === "tfoot") walk(child, false);
    }
  };
  walk(el, false);
  const table = [];
  for (const { tr, head } of rows) {
    const cells = tr.children.filter((kid) => typeof kid !== "string" && (kid.tag === "td" || kid.tag === "th"));
    if (!cells.length) continue;
    table.push({ header: head || cells.some((cell) => cell.tag === "th"), cells: cells.map((cell) => cellInline(cell, ctx, st)) });
  }
  return table.length ? { type: "table", rows: table } : null;
}

/// Verbatim text of a <pre>, without the first line break a browser drops.
const preText = (el) => textRaw(el).replace(/^\n/, "").replace(/\n+$/, "");

/// A paragraph is code when it has code text and every other token is whitespace
/// or a line break, with no links. Google Docs and Word write code blocks this way,
/// in a monospace font; a web page's lone <code> stays inline, since pages use <pre>.
const isCodeParagraph = (inl) =>
  inl.some((tok) => tok.kind === "text" && tok.m && tok.s.trim()) &&
  inl.every((tok) => tok.kind === "br" || (tok.kind === "text" && (tok.m || !tok.s.trim())));

/// The lines of a code paragraph, from its raw tokens, so that spaces are kept.
/// Each line loses its trailing whitespace.
const codeLines = (inl) => {
  let text = "";
  for (const tok of inl) {
    if (tok.kind === "br") text += "\n";
    else if (tok.kind === "text") text += tok.s.replace(/ /g, " ");
  }
  return text.split("\n").map((line) => line.replace(/\s+$/, ""));
};

/// Drops the empty lines at the start and end of a code block.
const trimBlankEdges = (lines) => {
  let start = 0;
  let end = lines.length;
  while (start < end && !lines[start]) start++;
  while (end > start && !lines[end - 1]) end--;
  return lines.slice(start, end);
};

/// Converts children to blocks. Inline runs become paragraphs; Word list
/// paragraphs in a row become one list; code paragraphs in a row become one pre block.
function blocksFrom(nodes, ctx, st) {
  const out = [];
  let pending = [];
  let run = [];
  // The pre block that the code paragraphs just before this node extend, with their lines.
  let code = null;
  const flushRun = () => {
    if (!run.length) return;
    out.push(...nestEntries(run));
    st.found = true;
    run = [];
  };
  const flush = () => {
    flushRun();
    const inl = normalize(pending);
    pending = [];
    if (inl.length) out.push({ type: "para", inl });
  };
  for (let index = 0; index < nodes.length; index++) {
    const node = nodes[index];
    if (typeof node === "string") {
      if (node.trim()) code = null;
      if (run.length) {
        if (!node.trim()) continue;
        flushRun();
      }
      pending.push(...inlinesOf(node, ctx, st));
      continue;
    }
    if (SKIP.has(node.tag)) continue;
    const previousCode = code;
    code = null;
    if (node.tag === "p" && /mso-list:\s*l\d/i.test(node.attrs.style ?? "")) {
      if (!run.length) flush();
      run.push(listEntry(node, ctx, st));
      continue;
    }
    if (node.tag === "br") {
      pending.push({ kind: "br" });
      continue;
    }
    flushRun();
    const inner = applyStyle(node, ctx);
    if (node.tag === "hr") flush();
    else if (CONTAINERS.has(node.tag)) {
      flush();
      out.push(...blocksFrom(node.children, inner, st));
    } else if (node.tag === "p" || node.tag === "li") {
      flush();
      const raw = node.children.flatMap((kid) => inlinesOf(kid, inner, st));
      if (node.tag === "p" && isCodeParagraph(raw)) {
        const lines = previousCode ? previousCode.lines : [];
        lines.push(...codeLines(raw));
        const block = previousCode ? previousCode.block : { type: "pre", text: "" };
        if (!previousCode) out.push(block);
        block.text = trimBlankEdges(lines).join("\n");
        st.found = true;
        code = { block, lines };
        continue;
      }
      const inl = normalize(raw);
      if (inl.length) out.push({ type: "para", inl });
    } else if (HEADING.test(node.tag)) {
      flush();
      const headingCtx = { ...inner, b: false, noBold: true };
      const inl = normalize(node.children.flatMap((kid) => inlinesOf(kid, headingCtx, st)));
      if (inl.length) {
        out.push({ type: "heading", level: Number(node.tag[1]), inl });
        st.found = true;
      }
    } else if (node.tag === "ul" || node.tag === "ol") {
      flush();
      // LibreOffice writes each nesting level as a sibling list, so the lists that
      // follow with only whitespace between them are read as one list.
      const group = [node];
      for (let next = index + 1; next < nodes.length; next++) {
        const sibling = nodes[next];
        if (typeof sibling === "string") {
          if (sibling.trim()) break;
          continue;
        }
        if (sibling.tag !== "ul" && sibling.tag !== "ol") break;
        group.push(sibling);
        index = next;
      }
      const lists = nestEntries(group.flatMap((list) => flattenList(list, 0, ctx, st)));
      if (lists.length) {
        st.found = true;
        out.push(...lists);
      }
    } else if (node.tag === "table") {
      flush();
      const table = tableBlock(node, ctx, st);
      if (table) {
        st.found = true;
        out.push(table);
      }
    } else if (node.tag === "blockquote") {
      flush();
      const inside = blocksFrom(node.children, inner, st);
      if (inside.length) {
        st.found = true;
        out.push({ type: "quote", blocks: inside });
      }
    } else if (node.tag === "pre") {
      flush();
      const text = preText(node);
      if (text.trim()) {
        st.found = true;
        out.push({ type: "pre", text });
      }
    } else if (containsBlock(node)) {
      flush();
      out.push(...blocksFrom(node.children, inner, st));
    } else {
      pending.push(...inlinesOf(node, ctx, st));
    }
  }
  flush();
  return out;
}

/* -------------------------------------------------------------- rendering */

const escLatex = (s) => s.replace(/[\\{}$&%#_~^]/g, (c) => LATEX_SPECIAL[c]);

/// Typst escapes its markup characters everywhere, and a few more at a line start.
const escTypst = (s, lineStart) => {
  let out = s.replace(/[\\*_#$@<[\]`~]/g, "\\$&");
  if (lineStart) {
    if (/^[=\-+/]/.test(s)) out = `\\${out}`;
    else if (/^\d+\./.test(s)) out = out.replace(/^(\d+)\./, "$1\\.");
  }
  return out;
};

/// Markdown (and Quarto) escapes its inline markup; a line start also gets heading, quote and list markers escaped.
/// Pandoc reads "@word" as a citation and "~x~" or "^x^" as sub- and superscript, so those are escaped too.
const escMarkdown = (s, lineStart) => {
  let out = s.replace(/[\\*_`[\]<>$@~^]/g, "\\$&");
  if (lineStart) {
    if (/^[#>+-]/.test(s)) out = `\\${out}`;
    else if (/^\d+[.)]/.test(s)) out = out.replace(/^(\d+)([.)])/, "$1\\$2");
  }
  return out;
};

const escapeText = (s, family, lineStart) => {
  if (family === "latex") return escLatex(s);
  if (family === "typst") return escTypst(s, lineStart);
  if (family === "markdown") return escMarkdown(s, lineStart);
  return s;
};

/// Emphasis around already-escaped text. Bold outside italic.
function wrap(text, tok, family) {
  if (family === "latex") {
    if (tok.b && tok.i) return `\\textbf{\\emph{${text}}}`;
    if (tok.b) return `\\textbf{${text}}`;
    return tok.i ? `\\emph{${text}}` : text;
  }
  if (family === "typst") {
    if (tok.b && tok.i) return `*_${text}_*`;
    if (tok.b) return `*${text}*`;
    return tok.i ? `_${text}_` : text;
  }
  if (family === "markdown") {
    if (tok.b && tok.i) return `***${text}***`;
    if (tok.b) return `**${text}**`;
    return tok.i ? `*${text}*` : text;
  }
  if (tok.b && tok.i) return `*/${text}/*`;
  if (tok.b) return `*${text}*`;
  return tok.i ? `/${text}/` : text;
}

/// Inline code. A backtick run in the code gets a longer fence.
function renderCode(s, family) {
  if (family === "latex") return `\\texttt{${escLatex(s)}}`;
  if (family === "org") return `~${s}~`;
  const longest = Math.max(0, ...(s.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  const pad = /^`|`$/.test(s) ? " " : "";
  return `${fence}${pad}${s}${pad}${fence}`;
}

/// A link. Text equal to the URL gets the bare form, which each format has.
/// A trailing slash is ignored on either side, since LibreOffice drops it from the text.
function renderLink(tok, family) {
  const href = tok.href;
  const noSlash = (s) => s.replace(/\/$/, "");
  const bare = noSlash(tokensText(tok.inl).trim()) === noSlash(href);
  const inner = renderInline(tok.inl, family, false);
  if (family === "latex") {
    const url = href.replace(/%/g, "\\%").replace(/#/g, "\\#");
    return bare ? `\\url{${url}}` : `\\href{${url}}{${inner}}`;
  }
  if (family === "typst") {
    const url = `"${href.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
    return bare ? `#link(${url})` : `#link(${url})[${inner}]`;
  }
  if (family === "markdown") {
    return bare ? `<${href}>` : `[${inner}](${href.replace(/ /g, "%20").replace(/\(/g, "%28").replace(/\)/g, "%29")})`;
  }
  // An Org description ends at the first "]", so any "]" in it is dropped.
  return bare ? `[[${href}]]` : `[[${href}][${inner.replace(/\n/g, " ").replace(/]/g, "")}]]`;
}

/// Source for inline tokens. A line start is the first token of a block, or the first after a line break.
function renderInline(inl, family, lineStart) {
  let out = "";
  let atLineStart = lineStart;
  for (const tok of inl) {
    if (tok.kind === "br") {
      out += BR[family];
      atLineStart = true;
      continue;
    }
    if (tok.kind === "link") out += renderLink(tok, family);
    else if (tok.c) out += renderCode(tok.s, family);
    else out += wrap(escapeText(tok.s, family, atLineStart && !(tok.b || tok.i)), tok, family);
    atLineStart = false;
  }
  return out;
}

const HEADING_MARK = { typst: "=", markdown: "#", org: "*" };

function renderHeading(block, family) {
  const text = renderInline(block.inl, family, false);
  if (family === "latex") return `\\${LATEX_SECTIONS[block.level - 1]}{${text}}`;
  return `${HEADING_MARK[family].repeat(block.level)} ${text}`;
}

function latexList(list, depth) {
  const env = list.ordered ? "enumerate" : "itemize";
  const pad = "  ".repeat(depth);
  const lines = [`${pad}\\begin{${env}}`];
  for (const item of list.items) {
    const text = renderInline(item.inl, "latex", false).replace(/\n/g, `\n${pad}    `);
    lines.push(`${pad}  \\item ${text}`);
    for (const sub of item.children) lines.push(latexList(sub, depth + 1));
  }
  lines.push(`${pad}\\end{${env}}`);
  return lines.join("\n");
}

function typstList(list, depth) {
  const pad = "  ".repeat(depth);
  const marker = list.ordered ? "+" : "-";
  const lines = [];
  for (const item of list.items) {
    const text = renderInline(item.inl, "typst", false).replace(/\n/g, `\n${pad}  `);
    lines.push(`${pad}${marker} ${text}`);
    for (const sub of item.children) lines.push(typstList(sub, depth + 1));
  }
  return lines.join("\n");
}

/// Markdown and Org lists: a nested list is indented by its parent's marker width.
function bulletLines(list, indent, family) {
  const lines = [];
  list.items.forEach((item, index) => {
    const marker = list.ordered ? `${(list.start ?? 1) + index}. ` : "- ";
    const pad = indent + " ".repeat(marker.length);
    const [first, ...rest] = renderInline(item.inl, family, false).split("\n");
    lines.push(indent + marker + first, ...rest.map((line) => (line ? pad + line : line)));
    for (const sub of item.children) lines.push(...bulletLines(sub, pad, family));
  });
  return lines;
}

function renderTable(table, family) {
  const width = Math.max(...table.rows.map((row) => row.cells.length));
  const rows = table.rows.map((row) => {
    const cells = row.cells.map((cell) => renderInline(cell, family, false));
    while (cells.length < width) cells.push("");
    return family === "markdown" ? cells.map((cell) => cell.replace(/\|/g, "\\|")) : cells;
  });
  const headed = table.rows[0].header;
  if (family === "latex") {
    const lines = [`\\begin{tabular}{${Array(width).fill("l").join(" ")}}`];
    rows.forEach((cells, index) => {
      lines.push(`${cells.join(" & ")} \\\\`);
      if (index === 0 && headed) lines.push("\\hline");
    });
    lines.push("\\end{tabular}");
    return lines.join("\n");
  }
  if (family === "typst") {
    const lines = ["#table(", `  columns: ${width},`];
    rows.forEach((cells, index) => {
      const row = cells.map((cell) => `[${cell}]`).join(", ");
      lines.push(index === 0 && headed ? `  table.header(${row}),` : `  ${row},`);
    });
    lines.push(")");
    return lines.join("\n");
  }
  // Pipe tables need a header row, so the first row is one even when it has no <th>.
  const row = (cells) => `| ${cells.join(" | ")} |`;
  const rule = family === "org" ? `|${Array(width).fill("---").join("+")}|` : `| ${Array(width).fill("---").join(" | ")} |`;
  return [row(rows[0]), rule, ...rows.slice(1).map(row)].join("\n");
}

function renderQuote(block, family) {
  const body = block.blocks.map((inner) => renderBlock(inner, family)).filter(Boolean).join("\n\n");
  if (family === "latex") return `\\begin{quote}\n${body}\n\\end{quote}`;
  if (family === "typst") return `#quote(block: true)[\n${body}\n]`;
  if (family === "org") return `#+begin_quote\n${body}\n#+end_quote`;
  return body
    .split("\n")
    .map((line) => (line ? `> ${line}` : ">"))
    .join("\n");
}

function renderPre(text, family) {
  if (family === "latex") return `\\begin{verbatim}\n${text}\n\\end{verbatim}`;
  if (family === "org") return `#+begin_src\n${text}\n#+end_src`;
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text}\n${fence}`;
}

/** @param {any} block @param {Family} family */
function renderBlock(block, family) {
  switch (block.type) {
    case "para":
      return renderInline(block.inl, family, true);
    case "heading":
      return renderHeading(block, family);
    case "list":
      if (family === "latex") return latexList(block, 0);
      if (family === "typst") return typstList(block, 0);
      return bulletLines(block, "", family).join("\n");
    case "table":
      return renderTable(block, family);
    case "quote":
      return renderQuote(block, family);
    case "pre":
      return renderPre(block.text, family);
    default:
      return "";
  }
}

/* ------------------------------------------------------------- entry points */

/// Source text in `format`, or null when the paste should be plain text: for
/// HTML and plain formats, and when the clipboard holds no formatting at all.
/** @param {Element} tree @param {string} format @returns {string | null} */
export function htmlToSource(tree, format) {
  const family = markupFamily(format);
  if (!family || family === "html") return null;
  const st = { found: false };
  const blocks = blocksFrom(tree.children ?? [], { b: false, i: false, c: false, noBold: false }, st);
  if (!st.found) return null;
  const text = blocks.map((block) => renderBlock(block, family)).filter((part) => part.length > 0).join("\n\n");
  return text || null;
}

/// The paste entry point. Any failure in parsing or conversion gives the plain text.
/** @returns {string} */
export function pasteText({ html, text, format, plain, parse = parseClipboardHtml }) {
  if (plain || !html) return text;
  try {
    return htmlToSource(parse(html), format) ?? text;
  } catch {
    return text;
  }
}
