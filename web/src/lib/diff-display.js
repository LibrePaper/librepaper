// The format-independent history display contract.
//
// Renderers produce HTML, but HTML is an implementation detail: wrapper
// elements, syntax-highlighting spans, generated ids, and whitespace used for
// layout must not become review changes. This module turns a rendered page
// into a small, serialisable semantic projection and compares two projections
// with deterministic resource bounds. It deliberately has no dependency on a
// renderer or on the live reader frame, which also makes the baseline safe to
// parse in an inert DOMParser document.

import { Segmenter } from "@formatjs/intl-segmenter";

export const PROJECTION_VERSION = 1;
// FormatJS bundles the UAX #29 tables and does not delegate to browser ICU.
// Keep the package/data identity explicit so segmentation changes invalidate
// projections rather than silently changing comparison results.
export const SEGMENTATION_VERSION = "@formatjs/intl-segmenter@12.2.7";
const WORD_SEGMENTER = new Segmenter("en", { granularity: "word" });
const GRAPHEME_SEGMENTER = new Segmenter("en", { granularity: "grapheme" });
export const DEFAULT_LIMITS = Object.freeze({
  tokens: 20_000,
  work: 2_000_000,
  trace: 4_000,
  hunks: 1_000,
  // Projection values cross the frame boundary. Count their UTF-16 payload
  // before walking locations so a small token count cannot hide a huge string.
  bytes: 16 * 1024 * 1024,
  path: 256,
  sourceOffset: 4 * 1024 * 1024,
  assetPath: 4_096,
  parts: 64,
  assets: 4_096,
  string: 256,
});

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

function assetEntries(value, limits = DEFAULT_LIMITS) {
  const paths = value?.paths;
  if (!paths || typeof paths !== "object" || Array.isArray(paths)) return [];
  const keys = Object.keys(paths);
  if (keys.length > limits.assets) return [];
  return keys.map((path) => {
    const entry = paths[path];
    if (path.length > limits.assetPath || !entry || typeof entry !== "object" || typeof entry.digest !== "string"
      || !/^[0-9a-f]{64}$/u.test(entry.digest)
      || (entry.url !== undefined && (typeof entry.url !== "string" || entry.url.length > limits.string * 4))) return null;
    return { path, digest: entry.digest, ...(entry.url ? { url: entry.url } : {}) };
  }).filter(Boolean);
}

function sourceUrl(element) {
  return element.getAttribute("src") || element.getAttribute("data-librepaper-source-url") || "";
}

function withoutFragment(value) {
  const at = String(value || "").indexOf("#");
  return at < 0 ? String(value || "") : String(value || "").slice(0, at);
}

function fragmentDigest(value) {
  const match = String(value || "").match(/#librepaper-asset=([0-9a-f]{64})$/u);
  return match ? match[1] : "";
}

// Renderer attributes are hints only. An image is comparable when its digest
// is backed by the captured tree and, when a URL/path is present, that locator
// agrees with the authorized evidence. This prevents stale or arbitrary
// `data-asset-digest` attributes from proving unchanged image bytes.
function assetIdentity(element, evidence) {
  const entries = assetEntries(evidence);
  if (!entries.length) return null;
  const path = element.getAttribute("data-asset-path") || "";
  const declared = element.getAttribute("data-asset-digest")
    || element.getAttribute("data-librepaper-image-digest")
    || element.getAttribute("data-librepaper-digest")
    || element.getAttribute("data-asset") || "";
  const url = sourceUrl(element);
  const fromFragment = fragmentDigest(url);
  const digest = declared || fromFragment;
  const base = withoutFragment(url);
  if (fromFragment && digest !== fromFragment) return null;
  const matches = entries.filter((entry) => {
    if (path && entry.path !== path) return false;
    if (digest && entry.digest !== digest) return false;
    if (base && entry.url && withoutFragment(entry.url) !== base) return false;
    // A URL without a matching captured URL is not an authorization proof.
    if (base && !entry.url) return false;
    // A bare renderer digest is not a path/digest mapping and is therefore
    // still untrusted, even when it happens to equal one captured digest.
    return Boolean(path || base);
  });
  if (matches.length !== 1) return null;
  return matches[0].digest;
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
      const identity = assetIdentity(element, state.assetEvidence);
      const alt = element.getAttribute("alt") || "Figure";
      atomic(state, element, nextPath, "figure", `asset:${identity || `unverified:${alt}`}|alt:${alt}`, alt, Boolean(identity));
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
      // Keep the original locator for evidence matching, then remove all
      // fetchable attributes before handing the inert markup to DOMParser.
      if (element.hasAttribute("src")) element.setAttribute("data-librepaper-source-url", element.getAttribute("src"));
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
    assetEvidence: { paths: Object.fromEntries(assetEntries(options.assetEvidence, limits).map((entry) => [entry.path, entry])) },
  };
  walk(state, root, []);
  flushRun(state);
  if (state.tokens.length > limits.tokens) {
    return { ...state, complete: false, incomplete: "token-limit" };
  }
  if (!state.complete) return state;
  return { ...state, complete: state.tokens.every((token) => token.comparable !== false) };
}

export function projectText(text) {
  // Source-only fallback keeps review useful if a renderer cannot produce
  // HTML. It is explicitly marked as such so callers never present it as a
  // semantic render comparison.
  const state = {
    version: PROJECTION_VERSION, segmentation: SEGMENTATION_VERSION, tokens: [], locations: [], text: "",
    code: true, run: [{ text: String(text || ""), path: [] }], runSpans: [],
    tokenLimit: DEFAULT_LIMITS.tokens, complete: true, sourceOnly: true,
  };
  flushRun(state);
  return { ...state, complete: state.complete, sourceOnly: true };
}

function equal(a, b) {
  return a?.comparable !== false && b?.comparable !== false && a?.kind === b?.kind && a?.value === b?.value;
}

// A bounded Myers frontier. Equal prefixes/suffixes are stripped before trace
// allocation. If the deterministic work/trace budget is exhausted the whole
// affected middle is returned as one simplified replacement.
export function diffProjections(a, b, options = {}) {
  const limits = boundedLimits(options.limits);
  const aa = a?.tokens || [], bb = b?.tokens || [];
  let fromA = 0; let fromB = 0;
  while (fromA < aa.length && fromB < bb.length && equal(aa[fromA], bb[fromB])) { fromA += 1; fromB += 1; }
  let toA = aa.length; let toB = bb.length;
  while (toA > fromA && toB > fromB && equal(aa[toA - 1], bb[toB - 1])) { toA -= 1; toB -= 1; }
  if (fromA === toA && fromB === toB) return [];
  const n = toA - fromA; const m = toB - fromB;
  if (n + m > limits.tokens) {
    return [{ fromA, toA, fromB, toB, deleted: aa.slice(fromA, toA), inserted: bb.slice(fromB, toB), simplified: true }];
  }
  const maxD = Math.min(n + m, Math.max(0, limits.trace));
  const size = 2 * maxD + 3;
  const center = maxD + 1;
  const frontier = new Int32Array(size);
  frontier.fill(-1);
  frontier[center + 1] = 0;
  const trace = [];
  let work = 0;
  let found = -1;
  for (let d = 0; d <= maxD; d += 1) {
    trace.push(frontier.slice());
    for (let k = -d; k <= d; k += 2) {
      const index = center + k;
      let x;
      if (k === -d || (k !== d && frontier[index - 1] < frontier[index + 1])) x = frontier[index + 1];
      else x = frontier[index - 1] + 1;
      let y = x - k;
      while (x < n && y < m && equal(aa[fromA + x], bb[fromB + y])) { x += 1; y += 1; work += 1; }
      work += 1;
      if (work > limits.work) break;
      frontier[index] = x;
      if (x >= n && y >= m) { found = d; break; }
    }
    if (found >= 0 || work > limits.work) break;
  }
  const coarse = () => [{ fromA, toA, fromB, toB, deleted: aa.slice(fromA, toA), inserted: bb.slice(fromB, toB), simplified: true }];
  if (found < 0) return coarse();

  // Reconstruct the shortest edit script from the saved frontiers.
  const edits = [];
  let x = n; let y = m;
  for (let d = found; d > 0; d -= 1) {
    const previous = trace[d];
    const k = x - y;
    const previousK = k === -d || (k !== d && previous[center + k - 1] < previous[center + k + 1]) ? k + 1 : k - 1;
    const previousX = previous[center + previousK];
    const previousY = previousX - previousK;
    while (x > previousX && y > previousY) { edits.push({ kind: "equal", a: x - 1, b: y - 1 }); x -= 1; y -= 1; }
    if (x === previousX) { edits.push({ kind: "insert", b: y - 1 }); y -= 1; }
    else { edits.push({ kind: "delete", a: x - 1 }); x -= 1; }
  }
  while (x > 0 && y > 0) { edits.push({ kind: "equal", a: x - 1, b: y - 1 }); x -= 1; y -= 1; }
  while (x > 0) { edits.push({ kind: "delete", a: x - 1 }); x -= 1; }
  while (y > 0) { edits.push({ kind: "insert", b: y - 1 }); y -= 1; }
  edits.reverse();

  const hunks = [];
  let hunk = null;
  const flush = () => { if (hunk) { hunks.push(hunk); hunk = null; } };
  let i = 0; let j = 0;
  for (const edit of edits) {
    if (edit.kind === "equal") { flush(); i += 1; j += 1; continue; }
    if (!hunk) hunk = { fromA: fromA + i, toA: fromA + i, fromB: fromB + j, toB: fromB + j, deleted: [], inserted: [] };
    if (edit.kind === "delete") { hunk.deleted.push(aa[fromA + i]); hunk.toA = fromA + (++i); }
    else { hunk.inserted.push(bb[fromB + j]); hunk.toB = fromB + (++j); }
  }
  flush();
  return hunks.length > limits.hunks ? coarse() : hunks.map((value) => ({ ...value, simplified: false }));
}

export function projectionHunks(a, b, options = {}) {
  const hunks = diffProjections(a, b, options);
  const sourceRanges = (projection, from, to) => (projection.locations || [])
    .slice(from, to)
    .flatMap((location) => location.parts || [])
    .filter((part) => typeof part.path === "string" && Number.isFinite(part.sourceStart)
      && Number.isFinite(part.sourceEnd) && part.sourceEnd > part.sourceStart)
    .map((part) => ({
      path: part.path, start: part.sourceStart, end: part.sourceEnd, validated: true,
    }));
  return hunks.map((hunk) => {
    const oldAt = a.tokens.slice(0, hunk.fromA).reduce((sum, token) => sum + (token.offset ?? (token.display || "").length), 0);
    const from = b.tokens.slice(0, hunk.fromB).reduce((sum, token) => sum + (token.offset ?? (token.display || "").length), 0);
    const insert = hunk.inserted.filter((token) => token.kind !== "block" && token.kind !== "figure")
      .map((token) => token.display || "").join("");
    const old = hunk.deleted.map((token) => token.display || "").join("");
    // Source ranges are display metadata only. They are consumed by the
    // source-only attribution pass after exact source evidence is established;
    // no text match or rendered intermediate can manufacture them.
    const deletedRanges = sourceRanges(a, hunk.fromA, hunk.toA);
    const insertedRanges = sourceRanges(b, hunk.fromB, hunk.toB);
    return {
      ...hunk, at: oldAt, position: from, insert, old, new: insert,
      sourceRanges: insertedRanges.length ? insertedRanges : deletedRanges,
      sourceRangesA: deletedRanges, sourceRangesB: insertedRanges,
      kind: old && insert ? "replace" : old ? "delete" : "insert",
    };
  });
}

export function validateProjection(projection, limits = DEFAULT_LIMITS) {
  limits = boundedLimits(limits);
  if (!projection || projection.version !== PROJECTION_VERSION || projection.segmentation !== SEGMENTATION_VERSION
    || !Array.isArray(projection.tokens)
    || projection.complete !== true || projection.tokens.length > limits.tokens || typeof projection.text !== "string"
    || projection.text.length * 2 > limits.bytes
    || (projection.sourceOnly !== undefined && typeof projection.sourceOnly !== "boolean")) return false;
  if (!Array.isArray(projection.locations) || projection.locations.length > limits.tokens * 2) return false;
  const rawAssets = projection.assetEvidence;
  if (rawAssets !== undefined && (!rawAssets || typeof rawAssets !== "object" || Array.isArray(rawAssets)
    || !rawAssets.paths || typeof rawAssets.paths !== "object" || Array.isArray(rawAssets.paths))) return false;
  const rawAssetPaths = rawAssets?.paths || {};
  if (assetEntries(rawAssets, limits).length !== Object.keys(rawAssetPaths).length) return false;
  let bytes = projection.text.length * 2;
  for (const entry of assetEntries(rawAssets, limits)) bytes += (entry.path.length + entry.digest.length + (entry.url?.length || 0)) * 2;
  let offsets = 0;
  const tokens = projection.tokens.every((token) => {
    if (!token || typeof token.kind !== "string" || token.kind.length > limits.string
      || typeof token.value !== "string" || typeof token.display !== "string"
      || token.comparable !== true || Number.isInteger(token.offset) === false
      || token.offset < 0 || token.offset > token.display.length) return false;
    bytes += (token.kind.length + token.value.length + token.display.length) * 2;
    offsets += token.offset;
    return bytes <= limits.bytes && offsets <= projection.text.length;
  });
  const validPath = (path) => Array.isArray(path) && path.length <= limits.path
    && path.every((index) => Number.isInteger(index) && index >= 0 && index <= limits.tokens * 4);
  const seenLocations = new Set();
  const locations = projection.locations.every((location) => {
    if (!location || !Number.isInteger(location.token) || location.token < 0 || location.token >= projection.tokens.length
      || seenLocations.has(location.token)
      || (location.element !== undefined && (typeof location.element !== "string" || location.element.length > limits.string))
      || (location.path !== undefined && !validPath(location.path))
      || (location.start !== undefined && (!Number.isInteger(location.start) || location.start < 0 || location.start > projection.text.length))
      || (location.end !== undefined && (!Number.isInteger(location.end) || location.end < 0 || location.end > projection.text.length))
      || (location.start !== undefined && location.end !== undefined && location.start > location.end)) return false;
    seenLocations.add(location.token);
    if (location.parts !== undefined && (!Array.isArray(location.parts) || location.parts.length > limits.parts)) return false;
    bytes += 16 + (location.element?.length || 0) * 2 + (location.path?.length || 0) * 4;
    const validParts = (location.parts || []).every((part) => part && validPath(part.path)
      && Number.isInteger(part.sourceStart) && Number.isInteger(part.sourceEnd)
      && part.sourceStart >= 0 && part.sourceEnd >= part.sourceStart
      && part.sourceEnd <= limits.sourceOffset);
    for (const part of location.parts || []) bytes += 16 + (part?.path?.length || 0) * 4;
    return validParts && bytes <= limits.bytes;
  });
  return tokens && locations && seenLocations.size === projection.tokens.length
    && offsets === projection.text.length && bytes <= limits.bytes;
}

export class ProjectionCache {
  constructor(limit = 16) { this.limit = limit; this.entries = new Map(); this.scope = ""; this.generation = 0; }
  setScope(scope) { if (String(scope) !== this.scope) { this.scope = String(scope); this.clear(); } }
  key(identity) { return `${this.scope}\u0000${JSON.stringify(identity)}`; }
  get(identity) { return this.entries.get(this.key(identity))?.value; }
  put(identity, value) {
    this.entries.set(this.key(identity), { value, generation: this.generation });
    while (this.entries.size > this.limit) this.entries.delete(this.entries.keys().next().value);
    return value;
  }
  clear() { this.generation += 1; this.entries.clear(); }
}
