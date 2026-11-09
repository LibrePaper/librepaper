import { formatOf } from "../formats.js";
import { includesOf } from "../opening.js";

// The part of a project one render actually reads.
//
// A project is a directory, but a page is one file: its includes, the
// bibliography and styles its compiler may name, and the figures those files
// point at. Handing the whole directory to the renderer makes a research
// corpus pay for every file on every file switch -- the wait this module
// removes. The selection is deliberately generous: if a path or its base name
// appears in a file the render can read, its figure is kept.

const SUPPORT = /\.(?:bib|csl|sty|cls|bst|clo|def|fd|cfg|bbx|cbx|lbx|lco|pgf|tikz|ist|ya?ml|json|csv|tsv|toml)$/i;

/** @param {{main?: string, texts?: Record<string, string>, digests?: Record<string, string>}} tree */
export function renderScope(tree) {
  const texts = tree?.texts || {};
  const digests = tree?.digests || {};
  const main = tree?.main;
  if (!main || !Object.hasOwn(texts, main)) return tree;

  const wanted = new Set([main]);
  const pending = [main];
  while (pending.length) {
    const path = pending.shift();
    for (const included of includesOf(path, texts)) {
      if (wanted.has(included)) continue;
      wanted.add(included);
      pending.push(included);
    }
  }
  // Compilers resolve bibliography and style files by names the source does
  // not spell as an include. They are small, and the project only keeps the
  // ones a paper can name.
  for (const path of Object.keys(texts)) {
    if (SUPPORT.test(path)) wanted.add(path);
  }
  // An authored HTML page embeds other HTML documents as frames rather than
  // including them, so the include walk above cannot see them. They are a
  // small set and the frame resolver needs their text.
  if (formatOf(main) === "html") {
    for (const path of Object.keys(texts)) {
      if (formatOf(path) === "html") wanted.add(path);
    }
  }

  const scopedTexts = {};
  for (const path of wanted) {
    if (Object.hasOwn(texts, path)) scopedTexts[path] = texts[path];
  }

  const bodies = Object.values(scopedTexts);
  const scopedDigests = {};
  for (const [path, sha] of Object.entries(digests)) {
    const base = path.slice(path.lastIndexOf("/") + 1);
    const stem = base.replace(/\.[^.]+$/, "");
    // A full path or a file name is exact. A stem is also allowed, because
    // LaTeX writes \includegraphics{plot} for plot.pdf, but only as a whole
    // word: "figure" must not match "figures/plot".
    const word = stem.length > 2
      ? new RegExp(`(^|[^\\p{L}\\p{N}_])${stem.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}([^\\p{L}\\p{N}_]|$)`, "u")
      : null;
    if (bodies.some((body) => body.includes(path) || body.includes(base) || word?.test(body))) {
      scopedDigests[path] = sha;
    }
  }

  return { ...tree, texts: scopedTexts, digests: scopedDigests };
}
