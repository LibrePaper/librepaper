// Pure fold arithmetic, kept free of imports so that node tests load it
// without CodeMirror or the message catalogue.

/** @typedef {{ from: number, to: number, kind: "preamble" | "include" | "comment" | "definitions", detail?: string, target?: string, others?: string }} Fold */

// The detail a pill shows is one line; longer text is cut to this many characters.
const DETAIL_LIMIT = 80;

/// Merges folds of the same kind that are separated only by whitespace, such as
/// a run of comment lines split by blank lines. Includes are never merged: each
/// one is its own file.
export function mergeFolds(text, folds) {
  const merged = [];
  for (const fold of folds) {
    const last = merged.at(-1);
    if (last && last.kind === fold.kind && fold.kind !== "include" && /^\s*$/.test(text.slice(last.to, fold.from))) {
      last.to = Math.max(last.to, fold.to);
      if (fold.detail) last.parts.push(fold.detail);
    } else {
      const { detail, ...rest } = fold;
      merged.push({ ...rest, parts: detail ? [detail] : [] });
    }
  }
  return merged.map(({ parts, ...fold }) => {
    const detail = joinDetails(parts);
    return detail ? { ...fold, detail } : fold;
  });
}

// The details joined by a middle dot, cut to DETAIL_LIMIT characters with an ellipsis.
function joinDetails(parts) {
  const joined = parts.join(" · ");
  if (joined.length <= DETAIL_LIMIT) return joined;
  return joined.slice(0, DETAIL_LIMIT - 1) + "…";
}
