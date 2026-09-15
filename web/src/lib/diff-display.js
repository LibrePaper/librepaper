// The format-independent projection of a rendered page.
//
// Renderers produce HTML, but HTML is an implementation detail: wrapper
// elements, syntax-highlighting spans, generated ids, and whitespace used for
// layout are not words anybody wrote. This module turns a rendered page into
// a small, bounded projection of the words in it, which is what a passage is
// looked for in. It deliberately has no dependency on a renderer or on the
// live reader frame, which also makes a historical page safe to parse in an
// inert DOMParser document.

import { Segmenter } from "@formatjs/intl-segmenter";

export const PROJECTION_VERSION = 1;
// FormatJS bundles the UAX #29 tables and does not delegate to browser ICU.
// Keep the package/data identity explicit so segmentation changes invalidate
// projections rather than silently changing their words.
export const SEGMENTATION_VERSION = "@formatjs/intl-segmenter@12.2.7";
const WORD_SEGMENTER = new Segmenter("en", { granularity: "word" });
const GRAPHEME_SEGMENTER = new Segmenter("en", { granularity: "grapheme" });
export const DEFAULT_LIMITS = Object.freeze({ tokens: 20_000 });

function boundedLimits(input) {
  const limits = { ...DEFAULT_LIMITS };
  for (const key of Object.keys(DEFAULT_LIMITS)) {
    const value = input?.[key];
    if (Number.isFinite(value)) limits[key] = Math.min(DEFAULT_LIMITS[key], Math.max(0, Math.floor(value)));
  }
  // Node count is an optional DOM-walk limit rather than part of the
  // serialized projection contract. Keep it bounded even when supplied by a
  // caller, so a hostile options object cannot turn projection into an
  // unbounded walk.
  if (Number.isFinite(input?.nodes)) limits.nodes = Math.min(DEFAULT_LIMITS.tokens * 4, Math.max(0, Math.floor(input.nodes)));
  return limits;
}

const BLOCKS = new Set([
  // Deliberately omit layout wrappers (`div`, `section`, `main`, and
  // navigation/header chrome). Only reader-visible semantic boundaries may
  // contribute a token.
  "ADDRESS", "BLOCKQUOTE", "CAPTION", "DD", "DL", "DT", "FIGCAPTION", "FIGURE",
  "H1", "H2", "H3", "H4", "H5", "H6", "HR", "LI", "OL", "P", "PRE", "TABLE", "TBODY",
  "TD", "TFOOT", "TH", "THEAD", "TR", "UL",
]);
const SKIP = new Set(["SCRIPT", "STYLE", "TEMPLATE"]);

function elementKey(element, path) {
  const explicit = element.getAttribute?.("data-librepaper-element")
    || element.getAttribute?.("data-source-key")
    || element.getAttribute?.("data-structural-key");
  return explicit ? String(explicit).slice(0, 256) : path.join(".");
}

function sourceParts(state, start, end) {
  return state.runSpans.filter((span) => span.end > start && span.start < end).map((span) => ({
    path: span.path,
    sourceStart: span.sourceStart + Math.max(0, start - span.start),
    sourceEnd: span.sourceStart + Math.min(span.end, end) - span.start,
  }));
}

function addToken(state, kind, value, display = value, location = {}, comparable = true, contributes = true) {
  if (!value && kind !== "block" && kind !== "break") return;
  if (state.tokens.length >= state.tokenLimit) {
    state.complete = false;
    state.incomplete = "token-limit";
    return;
  }
  const shown = String(display);
  const start = state.text.length;
  if (contributes) state.text += shown;
  const end = state.text.length;
  const token = { kind, value: String(value), display: shown, offset: contributes ? shown.length : 0, comparable };
  const index = state.tokens.length;
  state.tokens.push(token);
  state.locations.push({ token: index, start, end, ...location });
}

function wordIsLike(segment) {
  // FormatJS returns UAX #29 word records with isWordLike. The fallback is
  // only for older package builds; segmentation itself always comes from the
  // pinned package, never from host-provided segmentation.
  return typeof segment.isWordLike === "boolean"
    ? segment.isWordLike
    : /[\p{L}\p{N}\p{M}_]/u.test(segment.segment);
}

function emitRun(state, text, spans) {
  if (!text) return;
  // Check before invoking the pinned segmenter: a hostile single word can
  // consume substantial work without ever reaching the token-count limit.
  // Reject the whole projection rather than silently comparing a prefix.
  state.inputUnits = (state.inputUnits || 0) + text.length;
  if (text.length > 256 * 1024 || state.inputUnits > 1024 * 1024) {
    state.complete = false;
    state.incomplete = "input-limit";
    return;
  }
  state.runSpans = spans;
  let offset = 0;
  for (const segment of WORD_SEGMENTER.segment(text)) {
    if (state.tokens.length >= state.tokenLimit) {
      state.complete = false;
      state.incomplete = "token-limit";
      break;
    }
    const value = segment.segment;
    const start = segment.index ?? offset;
    const end = start + value.length;
    if (/^\s+$/u.test(value)) {
      const display = state.code ? value : " ";
      addToken(state, /\r|\n/u.test(display) ? "break" : "space", display, display,
        { parts: sourceParts(state, start, end), sourceStart: start, sourceEnd: end });
    } else if (wordIsLike(segment)) {
      addToken(state, "word", value, value,
        { parts: sourceParts(state, start, end), sourceStart: start, sourceEnd: end });
    } else {
      for (const grapheme of GRAPHEME_SEGMENTER.segment(value)) {
        const piece = grapheme.segment;
        const pieceStart = start + (grapheme.index ?? 0);
        const pieceEnd = pieceStart + piece.length;
        addToken(state, "grapheme", piece, piece,
          { parts: sourceParts(state, pieceStart, pieceEnd), sourceStart: pieceStart, sourceEnd: pieceEnd });
      }
    }
    offset = end;
  }
  state.runSpans = [];
}

function flushRun(state, blockBoundary = false) {
  if (!state.run.length) return;
  const text = state.run.map((part) => part.text).join("");
  // Formatting whitespace between block elements is not rendered prose.
  // Inert full-document parsing may discard the body's leading newline;
  // the live frame retains it. Neither may shift semantic token indices.
  if (blockBoundary && !state.code && /^\s*$/u.test(text)) {
    state.run = [];
    return;
  }
  let cursor = 0;
  const spans = state.run.map((part) => {
    const span = { start: cursor, end: cursor + part.text.length, path: part.path, sourceStart: 0 };
    cursor = span.end;
    return span;
  });
  emitRun(state, text, spans);
  state.run = [];
}

function canonicalMath(node) {
  if (!node) return "";
  if (node.nodeType === 3) return node.data.replace(/\s+/gu, " ");
  if (node.nodeType !== 1) return "";
  const attrs = [...node.attributes].filter((attribute) => !/^(?:class|style|id|aria-|data-)/iu.test(attribute.name))
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((attribute) => `${attribute.name}=${JSON.stringify(attribute.value)}`).join(",");
  return `<${node.localName || node.tagName.toLowerCase()}${attrs ? ` ${attrs}` : ""}>${[...node.childNodes].map(canonicalMath).join("")}</${node.localName || node.tagName.toLowerCase()}>`;
}

function normalizedMath(element) {
  const source = element.getAttribute("data-equation-source") || element.getAttribute("data-source")
    || (element.hasAttribute("data-math-style") ? element.textContent : "");
  const display = element.getAttribute("aria-label") || element.textContent || "Equation";
  if (source) return { value: `source:${source}`, display: source };
  const semantic = element.getAttribute("data-equation") || element.getAttribute("data-mathml")
    || canonicalMath(element.tagName.toLowerCase() === "math" ? element : element.querySelector("math") || element.querySelector("annotation[encoding='application/x-tex']"));
  if (semantic) return { value: `math:${semantic.replace(/\s+/gu, " ").trim()}`, display };
  return { value: `unverified-equation:${display}`, display, comparable: false };
}

function atomic(state, element, path, kind, value, display, comparable = true) {
  flushRun(state);
  addToken(state, kind, value, display, { element: elementKey(element, path), path }, comparable, kind !== "figure");
}

function walk(state, node, path = []) {
  if (!state.complete) return;
  if (node.nodeType === 3) {
    state.run.push({ text: node.data, path });
    return;
  }
  if (node.nodeType !== 1) return;
  state.nodes += 1;
  if (state.nodes > state.nodeLimit) { state.complete = false; state.incomplete = "node-limit"; return; }
  const element = node;
  const tag = element.tagName;
  if (SKIP.has(tag) || element.hidden || element.getAttribute("aria-hidden") === "true"
    || element.hasAttribute("data-librepaper-deletion") || element.hasAttribute("data-librepaper-synthetic")) {
    return;
  }
  const nextPath = path;
  const semantic = element.matches("[data-equation], [data-math-style], [data-citation], img, [role='math'], math");
  if (semantic) {
    if (tag === "IMG" || element.matches("figure img")) {
      flushRun(state);
      const alt = element.getAttribute("alt") || "Figure";
      atomic(state, element, nextPath, "figure", `alt:${alt}`, alt);
    } else if (element.matches("[data-citation]")) {
      const key = element.getAttribute("data-citation-key") || element.getAttribute("data-citation");
      const locator = element.getAttribute("data-locator") || "";
      atomic(state, element, nextPath, "citation", `citation:${key || `label:${element.textContent}`}|locator:${locator}`, element.textContent || "Citation", Boolean(key));
    } else {
      const math = normalizedMath(element);
      atomic(state, element, nextPath, "math", math.value, math.display, math.comparable !== false);
    }
    return;
  }
  const before = state.code;
  const codeBoundary = tag === "CODE" || tag === "PRE";
  if (codeBoundary) { flushRun(state, tag === "PRE"); state.code = true; }
  const boundary = BLOCKS.has(tag);
  if (boundary) {
    flushRun(state, true);
    addToken(state, "block", tag.toLowerCase(), `[${tag.toLowerCase()}]`,
      { element: elementKey(element, nextPath), path: nextPath }, true, false);
  }
  for (let index = 0; index < element.childNodes.length; index += 1) walk(state, element.childNodes[index], [...nextPath, index]);
  if (boundary || codeBoundary || tag === "BODY") flushRun(state, boundary || tag === "BODY");
  state.code = before;
  if (boundary && tag !== "BODY") addToken(state, "block", `/${tag.toLowerCase()}`, `[/${tag.toLowerCase()}]`,
    { element: elementKey(element, nextPath), path: nextPath }, true, false);
}

export function projectHtml(html, options = {}) {
  const parser = options.parser || globalThis.DOMParser;
  if (typeof parser !== "function") throw new Error("HTML projection requires DOMParser");
  let source = String(html || "");
  if (source.length > 4 * 1024 * 1024) throw new Error("Rendered comparison exceeds the projection size limit");
  // Template contents have no browsing context: remove resource attributes
  // before DOMParser sees them, rather than after it may initiate a fetch.
  if (globalThis.document?.createElement) {
    const template = document.createElement("template");
    template.innerHTML = source;
    for (const element of template.content.querySelectorAll("*")) {
      for (const attribute of ["src", "srcset", "href", "poster", "data", "style", "background"]) element.removeAttribute(attribute);
    }
    source = template.innerHTML;
  }
  const parsed = new parser().parseFromString(source, "text/html");
  return projectDom(parsed.body || parsed.documentElement, options);
}

// Preserve paths into the actual frame DOM, including skipped nodes.
export function projectDom(root, options = {}) {
  const limits = boundedLimits(options.limits);
  const state = {
    version: PROJECTION_VERSION, segmentation: SEGMENTATION_VERSION, tokens: [], locations: [], text: "",
    code: false, run: [], runSpans: [], nodes: 0,
    nodeLimit: limits.nodes ?? limits.tokens * 4,
    tokenLimit: limits.tokens, complete: true,
  };
  walk(state, root, []);
  flushRun(state);
  if (state.tokens.length > limits.tokens) {
    return { ...state, complete: false, incomplete: "token-limit" };
  }
  if (!state.complete) return state;
  return { ...state, complete: state.tokens.every((token) => token.comparable !== false) };
}
