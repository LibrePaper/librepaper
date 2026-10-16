// Near-miss names for package files the browser mirror does not have.
//
// A document that asks for a package TeX Live does not ship is most often
// making a typo, such as graphix for graphicx. The bundle index already lists
// every package the mirror has, so this file reads those names and picks the
// closest one. It has no imports and touches no engine, so the worker and the
// unit tests can both use it.

const decoder = new TextDecoder();

/**
 * The sorted, unique basenames of every `.sty` and `.cls` file in a bundle
 * index.
 *
 * @param {Uint8Array} indexBytes the JSON bytes of `bundles.json`
 * @returns {string[]}
 */
export function packageNames(indexBytes) {
  const index = JSON.parse(decoder.decode(indexBytes));
  const names = new Set();
  for (const key of Object.keys(index.files || {})) {
    const name = key.slice(key.lastIndexOf("/") + 1);
    if (/\.(sty|cls)$/.test(name)) names.add(name);
  }
  return [...names].sort();
}

/**
 * The closest package name to a requested file, or null.
 *
 * Only names with the same extension are candidates. A name with no
 * extension means `.sty`. The distance is optimal string alignment on the
 * stem, compared case-insensitively. A stem shorter than eight characters may
 * differ by one edit, a longer stem by two. Two edits on a short stem
 * suggest a real package for a file of the author's own, such as mystyle
 * for sistyle.
 *
 * @param {string} name the requested file name, such as `graphix.sty`
 * @param {string[]} names the package basenames the mirror has
 * @returns {string | null}
 */
export function nearestPackage(name, names) {
  const dot = name.lastIndexOf(".");
  const hasExtension = dot > 0;
  const full = hasExtension ? name : `${name}.sty`;
  if (names.includes(full)) return null;

  const stem = (hasExtension ? name.slice(0, dot) : name).toLowerCase();
  const ext = hasExtension ? name.slice(dot).toLowerCase() : ".sty";
  const limit = stem.length < 8 ? 1 : 2;

  let best = null;
  let bestDistance = Infinity;
  for (const candidate of names) {
    const candidateDot = candidate.lastIndexOf(".");
    if (candidateDot <= 0) continue;
    if (candidate.slice(candidateDot).toLowerCase() !== ext) continue;
    const candidateStem = candidate.slice(0, candidateDot).toLowerCase();
    if (Math.abs(candidateStem.length - stem.length) > limit) continue;

    const distance = optimalStringAlignment(stem, candidateStem);
    if (distance > limit) continue;
    if (distance < bestDistance || (distance === bestDistance && candidate < best)) {
      best = candidate;
      bestDistance = distance;
    }
  }
  return best;
}

// Levenshtein distance, plus one for swapping two adjacent characters.
function optimalStringAlignment(a, b) {
  const rows = a.length + 1;
  const cols = b.length + 1;
  const d = Array.from({ length: rows }, () => new Array(cols).fill(0));
  for (let i = 0; i < rows; i++) d[i][0] = i;
  for (let j = 0; j < cols; j++) d[0][j] = j;

  for (let i = 1; i < rows; i++) {
    for (let j = 1; j < cols; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) {
        d[i][j] = Math.min(d[i][j], d[i - 2][j - 2] + 1);
      }
    }
  }
  return d[rows - 1][cols - 1];
}
