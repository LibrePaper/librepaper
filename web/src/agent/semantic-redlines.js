// The frame-side half of the semantic redline contract.
//
// History owns the immutable projection and token hunks.  The frame must not
// trust either: both cross an untrusted document boundary and both are only
// useful for the particular target DOM/generation they describe.  This module
// contains the bounded validation and the small amount of re-projection needed
// before agent.js turns a hunk into DOM ranges.

import {
  DEFAULT_LIMITS,
  PROJECTION_VERSION,
  SEGMENTATION_VERSION,
  projectDom,
  validateProjection,
} from "../lib/diff-display.js";

export const SEMANTIC_REDLINE_VERSION = 1;
export const SEMANTIC_REDLINE_LIMITS = Object.freeze({
  ...DEFAULT_LIMITS,
  hunks: 1_000,
  deletedText: 100_000,
  hunkTokens: 20_000,
  string: 256,
});

const INT = (value) => Number.isInteger(value) && value >= 0;

function boundedString(value, limit = SEMANTIC_REDLINE_LIMITS.string) {
  return typeof value === "string" && value.length <= limit;
}

function validToken(token) {
  return token && typeof token === "object"
    && typeof token.kind === "string" && token.kind.length <= 64
    && typeof token.value === "string" && token.value.length <= SEMANTIC_REDLINE_LIMITS.deletedText
    && typeof token.display === "string" && token.display.length <= SEMANTIC_REDLINE_LIMITS.deletedText
    && typeof token.comparable === "boolean"
    && INT(token.offset) && token.offset <= token.display.length;
}

function validHunk(hunk, baselineCount, targetCount) {
  if (!hunk || typeof hunk !== "object") return false;
  if (!["insert", "delete", "replace"].includes(hunk.kind)) return false;
  if (!["fromA", "toA", "fromB", "toB"].every((key) => INT(hunk[key]))) return false;
  if (hunk.fromA > hunk.toA || hunk.toA > baselineCount || hunk.fromB > hunk.toB || hunk.toB > targetCount) return false;
  if (hunk.toA - hunk.fromA + hunk.toB - hunk.fromB > SEMANTIC_REDLINE_LIMITS.hunkTokens) return false;
  if (!Array.isArray(hunk.deleted) || !Array.isArray(hunk.inserted)
    || hunk.deleted.length !== hunk.toA - hunk.fromA
    || hunk.inserted.length !== hunk.toB - hunk.fromB
    || hunk.deleted.some((token) => !validToken(token))
    || hunk.inserted.some((token) => !validToken(token))) return false;
  if (hunk.simplified !== undefined && typeof hunk.simplified !== "boolean") return false;
  if (hunk.id !== undefined && !boundedString(String(hunk.id), 128)) return false;
  // These are display-only values, but validating them prevents an attacker
  // from using a legacy hunk field as an unbounded DOM attribute.
  for (const key of ["old", "insert", "new"]) {
    if (hunk[key] !== undefined && !boundedString(hunk[key], SEMANTIC_REDLINE_LIMITS.deletedText)) return false;
  }
  return true;
}

function validSuggestion(suggestion) {
  if (!suggestion || typeof suggestion !== "object") return false;
  if (suggestion.id !== undefined && !boundedString(String(suggestion.id), 128)) return false;
  if (suggestion.source !== undefined && typeof suggestion.source !== "object") return false;
  if (suggestion.overlap !== undefined && typeof suggestion.overlap !== "boolean") return false;
  if (suggestion.unlocatable !== undefined && typeof suggestion.unlocatable !== "boolean") return false;
  for (const key of ["proposed", "exact"]) {
    if (suggestion[key] !== undefined && !boundedString(suggestion[key], SEMANTIC_REDLINE_LIMITS.deletedText)) return false;
  }
  for (const key of ["start", "end", "targetStart", "targetEnd"]) {
    if (suggestion[key] !== undefined && !INT(suggestion[key])) return false;
  }
  return true;
}

// Validate the complete message, rather than filtering individual hunks.
// Silently retaining a valid prefix would paint an incomplete comparison and
// is particularly dangerous for replacements and overlapping suggestions.
export function validateSemanticRedlinePayload(payload, options = {}) {
  if (!payload || typeof payload !== "object") return false;
  if (payload.version !== SEMANTIC_REDLINE_VERSION) return false;
  if (!INT(payload.generation)) return false;
  if (payload.frameGeneration !== undefined && !INT(payload.frameGeneration)) return false;
  const projection = payload.targetProjection || payload.projection;
  if (!validateProjection(projection, options.limits || SEMANTIC_REDLINE_LIMITS)) return false;
  if (projection.version !== PROJECTION_VERSION || projection.segmentation !== SEGMENTATION_VERSION) return false;
  if (!Array.isArray(payload.hunks) || payload.hunks.length > SEMANTIC_REDLINE_LIMITS.hunks) return false;
  const baseline = payload.baselineProjection;
  if (baseline !== undefined && !validateProjection(baseline, options.limits || SEMANTIC_REDLINE_LIMITS)) return false;
  const baselineCount = baseline?.tokens?.length ?? SEMANTIC_REDLINE_LIMITS.tokens;
  if (!payload.hunks.every((hunk) => validHunk(hunk, baselineCount, projection.tokens.length))) return false;
  if (payload.suggestions !== undefined
    && (!Array.isArray(payload.suggestions)
      || payload.suggestions.length > SEMANTIC_REDLINE_LIMITS.hunks
      || !payload.suggestions.every(validSuggestion))) return false;
  if (payload.source !== undefined && !boundedString(String(payload.source), 128)) return false;
  return true;
}

function sameToken(a, b) {
  return a?.kind === b?.kind && a?.value === b?.value && a?.display === b?.display
    && a?.comparable === b?.comparable && a?.offset === b?.offset;
}

// Re-project a clone so no renderer markup or script can be executed.  The
// clone retains the target's semantic attributes and text but is detached from
// the live frame.  Annotation wrappers are unwrapped first because injected
// controls are not part of the canonical projection.
export function reprojectTarget(document, expected) {
  if (!document?.body || !validateProjection(expected, SEMANTIC_REDLINE_LIMITS)) return null;
  const actual = projectDom(document.body, {
    limits: SEMANTIC_REDLINE_LIMITS,
    // Reprojection must use the same authorized path/digest/URL evidence as
    // the inert endpoint projection. Without it, a valid captured figure
    // would become "unverified" merely because it crossed into the frame.
    assetEvidence: expected.assetEvidence,
  });
  if (!validateProjection(actual, SEMANTIC_REDLINE_LIMITS)) return null;
  if (actual.version !== expected.version || actual.segmentation !== expected.segmentation) return null;
  if (actual.tokens.length !== expected.tokens.length || actual.text !== expected.text) return null;
  for (let index = 0; index < expected.tokens.length; index += 1) {
    if (!sameToken(actual.tokens[index], expected.tokens[index])) return null;
  }
  return actual;
}

export function displayOffset(projection, tokenIndex) {
  if (!projection || !INT(tokenIndex) || tokenIndex > projection.tokens.length) return null;
  let offset = 0;
  for (let index = 0; index < tokenIndex; index += 1) offset += projection.tokens[index].offset;
  return offset;
}

export function tokenSpan(projection, tokenIndex) {
  if (!projection || !INT(tokenIndex) || tokenIndex >= projection.tokens.length) return null;
  const start = displayOffset(projection, tokenIndex);
  if (start === null) return null;
  return { start, end: start + projection.tokens[tokenIndex].offset };
}

export function hunkDisplayRange(projection, hunk) {
  if (!projection || !hunk || !INT(hunk.fromB) || !INT(hunk.toB)
    || hunk.fromB > hunk.toB || hunk.toB > projection.tokens.length) return null;
  return {
    start: displayOffset(projection, hunk.fromB),
    end: displayOffset(projection, hunk.toB),
  };
}

export function targetSemanticItems(payload) {
  const projection = payload.targetProjection || payload.projection;
  if (!projection) return [];
  const items = [];
  for (let index = 0; index < payload.hunks.length; index += 1) {
    const hunk = payload.hunks[index];
    const range = hunkDisplayRange(projection, hunk);
    if (!range) return null;
    const id = String(hunk.id ?? `semantic-hunk-${index}`);
    const inserted = hunk.inserted || [];
    const deleted = hunk.deleted || [];
    if (hunk.kind === "insert" || hunk.kind === "replace") {
      items.push({
        id,
        kind: "insert",
        start: range.start,
        end: range.end,
        fromB: hunk.fromB,
        toB: hunk.toB,
        tokens: inserted,
        who: hunk.who || "",
        author: hunk.author,
      });
    }
    if (hunk.kind === "delete" || hunk.kind === "replace") {
      const text = deleted.map((token) => token?.display || "").join("");
      if (text.length > SEMANTIC_REDLINE_LIMITS.deletedText) return null;
      items.push({
        id,
        kind: "delete",
        at: range.start,
        text,
        fromB: hunk.fromB,
        toB: hunk.toB,
        tokens: deleted,
        who: hunk.who || "",
        author: hunk.author,
      });
    }
  }
  return items;
}
