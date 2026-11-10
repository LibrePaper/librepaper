// LaTeX-style equation numbering for TeX handed to KaTeX, which has no
// \label, \ref or \eqref and no multline. Pure string logic: the document
// edges live in math.js.
//
// Numbered like LaTeX: equation and multline get one number; align, gather,
// eqnarray and flalign get one per row, except rows with \nonumber or
// \notag. Starred environments, displaymath and bare $$ get none, and a
// formula (or row) that already has a \tag keeps it and is not counted.

const ENV = /^\s*\\begin\{([A-Za-z]+\*?)\}([\s\S]*)\\end\{\1\}\s*$/;
const LABEL = /\\label\s*\{([^{}]*)\}/g;
const TAG = /\\tag\*?\s*\{/;
const NOTAG = /\\(?:nonumber|notag)(?![A-Za-z])/;
const NOTAGS = /\\(?:nonumber|notag)(?![A-Za-z])/g;
const ROW_ENVS = new Set(["align", "flalign", "gather", "eqnarray"]);

/// Splits an environment body into rows on top-level `\\`.
export function splitRows(body) {
  const rows = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (c === "\\") {
      if (body[i + 1] === "\\") {
        if (depth === 0) {
          rows.push(body.slice(start, i));
          start = i + 2;
        }
        i++;
      } else {
        const name = /^[A-Za-z]+/.exec(body.slice(i + 1))?.[0];
        if (name === "begin") depth++;
        else if (name === "end") depth--;
        else if (!name) i++; // \{ \& and friends
        if (name) i += name.length;
      }
    }
  }
  rows.push(body.slice(start));
  return rows;
}

/**
 * Rewrites one display formula for KaTeX. `claim(keys)` is called once per
 * numbered equation or row, with the labels found in it, and returns its
 * number. Returns the TeX to render and every label key found.
 */
export function rewriteDisplay(source, claim) {
  const keys = [];
  const take = (text) => {
    for (const found of text.matchAll(LABEL)) keys.push(found[1].trim());
    return text.replace(LABEL, "");
  };
  const tagged = (text, from) => {
    if (TAG.test(text)) return text;
    const tail = /\s*$/.exec(text)[0];
    return `${text.slice(0, text.length - tail.length)}\\tag{${claim(keys.slice(from))}}${tail}`;
  };

  const match = ENV.exec(source);
  if (!match) return { tex: take(source), keys };
  const [, name, body] = match;
  const starred = name.endsWith("*");
  const base = starred ? name.slice(0, -1) : name;

  if (base === "displaymath") return { tex: take(body), keys };

  if (base === "equation" || base === "multline") {
    const from = keys.length;
    let text = take(body).replace(NOTAGS, "");
    if (!starred) text = tagged(text, from);
    // KaTeX has no multline: gather* is the nearest.
    const tex = base === "multline" ? `\\begin{gather*}${text}\\end{gather*}` : text;
    return { tex, keys };
  }

  if (ROW_ENVS.has(base)) {
    const target = base === "gather" ? "gather*" : "align*";
    const rows = splitRows(body).map((row) => {
      const from = keys.length;
      let text = take(row);
      const skip = NOTAG.test(text);
      text = text.replace(NOTAGS, "");
      // eqnarray has a cell on each side of the relation; align one.
      if (base === "eqnarray") text = text.replace(/^([^&]*&[^&]*)&/, "$1");
      if (!starred && !skip && text.trim() !== "") text = tagged(text, from);
      return text;
    });
    return { tex: `\\begin{${target}}${rows.join("\\\\")}\\end{${target}}`, keys };
  }

  // Anything else is KaTeX's own (or unknown to it): only labels go.
  return { tex: take(source), keys };
}

/**
 * Numbers a document's display formulas, given their TeX in order. Returns
 * `tex` (what to render, per formula), `keys` (labels found, per formula)
 * and `labels` (label key to number; the first use of a key wins).
 */
export function numberDisplays(sources) {
  const labels = new Map();
  let counter = 0;
  const claim = (keys) => {
    const number = ++counter;
    for (const key of keys) if (!labels.has(key)) labels.set(key, number);
    return number;
  };
  const tex = [];
  const keys = [];
  for (const source of sources) {
    const done = rewriteDisplay(source, claim);
    tex.push(done.tex);
    keys.push(done.keys);
  }
  return { tex, keys, labels };
}

/// `\ref{k}` or `\eqref{k}` as a whole formula: what it is and the key.
export function parseReference(tex) {
  const found = /^\s*\\(ref|eqref)\s*\{([^{}]*)\}\s*$/.exec(tex);
  return found ? { kind: found[1], key: found[2].trim() } : null;
}
