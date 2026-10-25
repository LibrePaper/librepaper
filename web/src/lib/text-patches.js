// Character patches between two texts.

import { diff } from "@codemirror/merge";

const encoder = new TextEncoder();

function isHigh(code) {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLow(code) {
  return code >= 0xdc00 && code <= 0xdfff;
}

/// The edits that turn `from` into `to`, as patches on UTF-8 byte offsets of
/// `from`. The character diff works in UTF-16 units, so a boundary that falls
/// inside a surrogate pair moves outward to keep every patch whole characters.
export function textPatches(path, from, to) {
  const patches = [];
  let units = 0;
  let bytes = 0;
  const at = (index) => {
    bytes += encoder.encode(from.slice(units, index)).length;
    units = index;
    return bytes;
  };
  for (const change of diff(from, to)) {
    let { fromA, toA, fromB, toB } = change;
    if (
      fromA > 0
      && isHigh(from.charCodeAt(fromA - 1))
      && (isLow(from.charCodeAt(fromA)) || isLow(to.charCodeAt(fromB)))
    ) {
      fromA -= 1;
      fromB -= 1;
    }
    if (
      toA > 0
      && isHigh(from.charCodeAt(toA - 1))
      && (isLow(from.charCodeAt(toA)) || isLow(to.charCodeAt(toB)))
    ) {
      toA += 1;
      toB += 1;
    }
    // Widening can make a hunk reach back into the one before it; the two
    // then become one patch.
    const last = patches.at(-1);
    if (last && fromA < units) {
      last.end = at(Math.max(toA, units));
      last.exact = from.slice(last.fromA, units);
      last.replacement = to.slice(last.fromB, Math.max(toB, last.toB));
      last.toB = Math.max(toB, last.toB);
      continue;
    }
    const start = at(fromA);
    const end = at(toA);
    patches.push({ path, start, end, exact: from.slice(fromA, toA), replacement: to.slice(fromB, toB), fromA, fromB, toB });
  }
  return patches.map(({ path, start, end, exact, replacement }) => ({ path, start, end, exact, replacement }));
}
