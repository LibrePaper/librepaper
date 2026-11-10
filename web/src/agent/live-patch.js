// Applies one live patch to the rendered page inside the document frame.
// Dependency-free. Conservative by design: on any ambiguity nothing changes
// and the function returns false.

const SKIP = "math, .ltx_Math, .ltx_equation, .katex, [data-lp-tex], .ltx_note_outer, .ltx_note_content, script, style";

// "T:L:C-T:L:C" or "T:L:C" -> { tag, startLine, endLine }; the start and end
// tags must match.
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

const covers = (position, tag, line) => position && position.tag === tag
  && position.startLine <= line && line <= position.endLine;

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

  let candidates = [];
  for (const element of document.querySelectorAll("[data-sourcepos]")) {
    if (element.closest(SKIP)) continue;
    if (covers(parseSourcepos(element.getAttribute("data-sourcepos")), tag, line)) candidates.push(element);
  }
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

const squash = (text) => String(text).replace(/\s+/g, "");

function applyMath(document, sources, katex, patch) {
  const { file, line, oldTex, tex, display } = patch;
  if (typeof file !== "string" || typeof oldTex !== "string" || typeof tex !== "string") return false;
  if (!Number.isInteger(line) || tex === "" || !katex || typeof katex.renderToString !== "function") return false;
  const tag = Array.isArray(sources) ? sources.indexOf(file) : -1;
  if (tag < 0) return false;

  const wanted = squash(oldTex);
  const matches = [];
  for (const element of document.querySelectorAll("math, [data-lp-tex]")) {
    // A KaTeX rendering inside a live span is part of that span.
    if (element.parentElement && element.parentElement.closest("[data-lp-tex]")) continue;
    let positioned = element.hasAttribute("data-sourcepos") ? element : element.closest("[data-sourcepos]");
    if (!positioned) continue;
    if (!covers(parseSourcepos(positioned.getAttribute("data-sourcepos")), tag, line)) continue;
    const source = element.hasAttribute("data-lp-tex") ? element.getAttribute("data-lp-tex") : element.getAttribute("alttext");
    if (source === null || squash(source) !== wanted) continue;
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

// Returns true only if the page was changed.
export function applyLivePatch(document, { sources, katex } = {}, patch) {
  try {
    if (!document || !patch || typeof patch !== "object") return false;
    if (patch.kind === "prose") return applyProse(document, sources, patch);
    if (patch.kind === "math") return applyMath(document, sources, katex, patch);
  } catch {
    return false;
  }
  return false;
}
