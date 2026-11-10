// Applies one live patch to the rendered page inside the document frame.
// Dependency-free. Conservative by design: on any ambiguity nothing changes
// and the function returns false.
//
// The engine's data-sourcepos attributes carry only where a construct starts
// ("T:L:C-T:L:C" with the same start and end, e.g. "0:12:2-0:12:2" for a
// paragraph spanning lines 12-14). So a prose element is taken to own every
// line from its start line up to the next element that starts. Math alttext is
// normalized TeX (`h_t` becomes `h_{t}`, `x^2` becomes `x^{2}`), so math is
// compared with whitespace and braces removed.

// Elements inside these are not prose: math, its renderings, footnotes, scripts.
const SKIP = "math, .ltx_Math, .ltx_equation, .katex, [data-lp-tex], .ltx_note_outer, .ltx_note_content, script, style";

// "T:L:C-T:L:C" or "T:L:C" -> { tag, startLine, endLine }; the start and end
// tags must match. endLine equals startLine when only the start is given.
export function parseSourcepos(value) {
  if (typeof value !== "string") return null;
  const match = /^(\d+):(\d+):(\d+)(?:-(\d+):(\d+):(\d+))?$/.exec(value.trim());
  if (!match) return null;
  const tag = Number(match[1]);
  const startLine = Number(match[2]);
  if (match[4] === undefined) return { tag, startLine, endLine: startLine };
  if (Number(match[4]) !== tag) return null;
  return { tag, startLine, endLine: Number(match[5]) };
}

// The patch line is where the math node starts in the source, but LaTeXML may
// point a display environment at its content line, so allow a little slack
// before and after.
const startsNear = (position, tag, line) => position && position.tag === tag
  && line - 1 <= position.startLine && position.startLine <= line + 3;

const isWhitespace = (character) => character === " " || character === "\t" || character === "\n" || character === "\r";

function normalize(text) {
  let out = "";
  let space = false;
  for (const character of text) {
    if (isWhitespace(character)) {
      if (!space) out += " ";
      space = true;
    } else {
      out += character;
      space = false;
    }
  }
  return out;
}

// Normalized text of an element plus, for every normalized character, the
// text node and the data range it came from.
function collect(root) {
  let text = "";
  const map = [];
  let space = false;
  const walk = (node) => {
    for (let child = node.firstChild; child; child = child.nextSibling) {
      if (child.nodeType === 1) {
        if (!child.matches(SKIP)) walk(child);
      } else if (child.nodeType === 3) {
        const data = child.data;
        for (let index = 0; index < data.length; index++) {
          if (isWhitespace(data[index])) {
            if (space) continue;
            let end = index + 1;
            while (end < data.length && isWhitespace(data[end])) end++;
            text += " ";
            map.push({ node: child, start: index, end });
            space = true;
          } else {
            text += data[index];
            map.push({ node: child, start: index, end: index + 1 });
            space = false;
          }
        }
      }
    }
  };
  walk(root);
  return { text, map };
}

function applyProse(document, sources, patch) {
  const { file, line, before, deleted, inserted, after } = patch;
  if (![file, before, deleted, inserted, after].every((value) => typeof value === "string")) return false;
  if (!Number.isInteger(line)) return false;
  const tag = Array.isArray(sources) ? sources.indexOf(file) : -1;
  if (tag < 0) return false;
  if (deleted === "" && inserted === "") return false;

  // The owning element is the one that starts closest above the patch line.
  const reachable = [];
  let latest = -1;
  for (const element of document.querySelectorAll("[data-sourcepos]")) {
    if (element.closest(SKIP)) continue;
    const position = parseSourcepos(element.getAttribute("data-sourcepos"));
    if (!position || position.tag !== tag || position.startLine > line) continue;
    if (position.endLine > position.startLine && line > position.endLine) continue;
    reachable.push({ element, startLine: position.startLine });
    latest = Math.max(latest, position.startLine);
  }
  // An inline element (an \emph span, a citation) can start later on the
  // paragraph's lines than the paragraph itself; the edit may be in the plain
  // text around it, so search its enclosing block.
  const BLOCK = "p, h1, h2, h3, h4, h5, h6, li, td, th, figcaption, dt, dd";
  let candidates = [...new Set(reachable.filter((entry) => entry.startLine === latest)
    .map((entry) => entry.element.closest(BLOCK) || entry.element))];
  // Keep the deepest of those, so a nested wrapper with the same start loses.
  candidates = candidates.filter((element) => !candidates.some((other) => other !== element && element.contains(other)));

  const needle = normalize(before + deleted + after);
  if (needle === "") return false;
  const deleteStart = normalize(before).length;
  const deleteEnd = normalize(before + deleted).length;
  let found = null;
  let count = 0;
  for (const candidate of candidates) {
    const { text, map } = collect(candidate);
    for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + 1)) {
      count++;
      found = { map, at };
      if (count > 1) return false;
    }
  }
  if (count !== 1) return false;
  const { map, at } = found;

  if (deleteEnd > deleteStart) {
    const first = map[at + deleteStart];
    const last = map[at + deleteEnd - 1];
    if (!first || !last || first.node !== last.node) return false;
    first.node.replaceData(first.start, last.end - first.start, inserted);
    return true;
  }
  // Pure insertion: right after the last context character, else right
  // before the first following one.
  if (deleteStart > 0) {
    const previous = map[at + deleteStart - 1];
    if (!previous) return false;
    previous.node.insertData(previous.end, inserted);
    return true;
  }
  const next = map[at];
  if (!next) return false;
  next.node.insertData(next.start, inserted);
  return true;
}

// Alttext is normalized TeX, so whitespace and braces are not significant.
const comparableTex = (text) => String(text).replace(/[\s{}]/g, "");

function applyMath(document, sources, katex, patch) {
  const { file, line, oldTex, tex, display } = patch;
  if (typeof file !== "string" || typeof oldTex !== "string" || typeof tex !== "string") return false;
  if (!Number.isInteger(line) || tex === "" || !katex || typeof katex.renderToString !== "function") return false;
  const tag = Array.isArray(sources) ? sources.indexOf(file) : -1;
  if (tag < 0) return false;

  const wanted = comparableTex(oldTex);
  const matches = [];
  for (const element of document.querySelectorAll("math, [data-lp-tex]")) {
    // A KaTeX rendering inside a live span is part of that span.
    if (element.parentElement && element.parentElement.closest("[data-lp-tex]")) continue;
    let positioned = element.hasAttribute("data-sourcepos") ? element : element.closest("[data-sourcepos]");
    if (!positioned) continue;
    if (!startsNear(parseSourcepos(positioned.getAttribute("data-sourcepos")), tag, line)) continue;
    const source = element.hasAttribute("data-lp-tex") ? element.getAttribute("data-lp-tex") : element.getAttribute("alttext");
    if (source === null || comparableTex(source) !== wanted) continue;
    matches.push(element);
  }
  if (matches.length !== 1) return false;
  const target = matches[0];

  let html;
  try {
    html = katex.renderToString(tex, { displayMode: Boolean(display), throwOnError: true, output: "htmlAndMathml" });
  } catch {
    return false;
  }
  if (typeof html !== "string" || html === "") return false;
  const span = document.createElement("span");
  span.className = "lp-live-math";
  span.setAttribute("data-lp-tex", tex);
  span.setAttribute("data-lp-provisional", "");
  if (display) span.setAttribute("style", "display:block;text-align:center");
  if (target.hasAttribute("data-sourcepos")) span.setAttribute("data-sourcepos", target.getAttribute("data-sourcepos"));
  span.innerHTML = html;
  target.replaceWith(span);
  return true;
}

const BLOCK_TAGS = "p, h1, h2, h3, h4, h5, h6, li, td, figcaption, dt, dd";

// A paragraph the last compile has not seen: shown as a provisional paragraph
// after the block that owns the anchor line, and updated on every keystroke.
function applyParagraph(document, sources, patch) {
  const { file, anchorLine, text } = patch;
  if (typeof file !== "string" || typeof text !== "string" || !Number.isInteger(anchorLine)) return false;
  const tag = Array.isArray(sources) ? sources.indexOf(file) : -1;
  if (tag < 0) return false;
  const key = `${tag}:${anchorLine}`;
  const existing = [...document.querySelectorAll("[data-lp-para]")].find((element) => element.getAttribute("data-lp-para") === key);
  if (existing) {
    if (text === "") {
      existing.remove();
      return true;
    }
    const target = existing.querySelector("p");
    if (!target || target.textContent === text) return false;
    target.textContent = text;
    return true;
  }
  if (text === "") return false;

  let anchor = null;
  let latest = -1;
  for (const element of document.querySelectorAll(BLOCK_TAGS)) {
    if (element.closest(SKIP)) continue;
    const position = parseSourcepos(element.getAttribute("data-sourcepos"));
    if (!position || position.tag !== tag || position.startLine > anchorLine) continue;
    if (position.startLine >= latest) {
      latest = position.startLine;
      anchor = element;
    }
  }
  if (!anchor) return false;
  const container = anchor.closest(".ltx_para") || anchor;
  const div = document.createElement("div");
  div.className = "ltx_para";
  div.setAttribute("data-lp-provisional", "");
  div.setAttribute("data-lp-para", key);
  const paragraph = document.createElement("p");
  paragraph.className = "ltx_p";
  paragraph.textContent = text;
  div.appendChild(paragraph);
  container.after(div);
  return true;
}

// Returns true only if the page was changed.
/**
 * @param {Document} document
 * @param {{ sources?: Array<string | null> | null, katex?: any }} context
 * @param {any} patch
 */
export function applyLivePatch(document, { sources, katex } = {}, patch) {
  try {
    if (!document || !patch || typeof patch !== "object") return false;
    if (patch.kind === "prose") return applyProse(document, sources, patch);
    if (patch.kind === "paragraph") return applyParagraph(document, sources, patch);
    if (patch.kind === "math") return applyMath(document, sources, katex, patch);
  } catch {
    return false;
  }
  return false;
}
