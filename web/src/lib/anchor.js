// Client-side re-anchoring. A port of reanchor_comments() from the Flask app,
// moved from upload time to read time: the stored {exact, prefix, suffix}
// selector is the source of truth and offsets are derived on every load. That
// is what lets a document be replaced by a plain object write, with no
// migration step and no server-side knowledge of the HTML.

export function commonPrefix(a, b) {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  return i;
}

const reverse = (value) => [...value].reverse().join("");

// Matches JavaScript's regex `\s` class exactly: ASCII whitespace, the
// vertical tab, NBSP, the Unicode space separators, line/paragraph
// separators, and BOM/ZWNBSP.
function isSpace(code) {
  switch (code) {
    case 0x09: // \t
    case 0x0a: // \n
    case 0x0b: // \v
    case 0x0c: // \f
    case 0x0d: // \r
    case 0x20: // space
    case 0xa0: // NBSP
    case 0x1680: // ogham space mark
    case 0x2028: // line separator
    case 0x2029: // paragraph separator
    case 0x202f: // narrow no-break space
    case 0x205f: // medium mathematical space
    case 0x3000: // ideographic space
    case 0xfeff: // BOM / zero width no-break space
      return true;
    default:
      return code >= 0x2000 && code <= 0x200a; // en quad .. hair space
  }
}

/**
 * A whitespace-flattened view of `text`, with a map back to the original
 * offsets. Runs of whitespace become one space, so a quote survives a
 * document being reflowed -- and so do the quotes stored by earlier versions
 * of the reader, which took the selection through the browser's own
 * whitespace normalisation before saving it.
 */
export function flatten(text) {
  const chunks = [];
  const map = new Uint32Array(text.length);
  let mapLen = 0;
  let outLen = 0;
  let space = false;
  for (let i = 0; i < text.length; i++) {
    if (isSpace(text.charCodeAt(i))) {
      if (!space && outLen) {
        chunks.push(" ");
        map[mapLen++] = i;
        outLen++;
        space = true;
      }
      continue;
    }
    chunks.push(text[i]);
    map[mapLen++] = i;
    outLen++;
    space = false;
  }
  return { text: chunks.join(""), map: map.subarray(0, mapLen) };
}

const flat = (value) => value.replace(/\s+/g, " ").trim();

/**
 * Locate one selector in `text`.
 * Returns {start, end}, or null when the passage is nowhere to be found.
 * `view` is the flattened text from `flatten`, used only as a fallback.
 */
export function anchorOne(text, selector, view = null) {
  const found = search(text, selector);
  if (found) return found;
  if (!view) return null;
  // Nothing matched the document as written. Try again with whitespace
  // flattened on both sides, then map the hit back to real offsets.
  const loose = search(view.text, {
    exact: flat(selector.exact || ""),
    prefix: flat(selector.prefix || ""),
    suffix: flat(selector.suffix || ""),
    position: null,
  });
  if (!loose || loose.end === loose.start) return null;
  return { start: view.map[loose.start], end: view.map[loose.end - 1] + 1 };
}

function search(text, { exact, prefix = "", suffix = "", position = null }) {
  if (!exact) return null;
  const positions = [];
  for (let cursor = text.indexOf(exact); cursor >= 0; cursor = text.indexOf(exact, cursor + 1)) {
    positions.push(cursor);
    if (positions.length > 5000) break;
  }
  if (positions.length === 0) return null;

  let selected = null;
  if (positions.length === 1) {
    selected = positions[0];
  } else {
    // Score each candidate by how much of the recorded context still matches
    // on either side, and prefer a strict winner.
    const scored = positions
      .map((candidate) => {
        const before = text.slice(Math.max(0, candidate - prefix.length), candidate);
        const after = text.slice(candidate + exact.length, candidate + exact.length + suffix.length);
        return {
          candidate,
          score: commonPrefix(reverse(prefix), reverse(before)) + commonPrefix(suffix, after),
        };
      })
      .sort((a, b) => b.score - a.score);
    if (scored[0].score > scored[1].score) {
      selected = scored[0].candidate;
    } else {
      // A document that repeats itself gives its passages identical context,
      // and the score cannot separate them. The offset recorded when the
      // comment was made says which copy was meant. Comments made before that
      // was recorded fall back to the first of the tied candidates, which is
      // a guess -- but the passage and its context read the same either way,
      // and it beats dropping the comment as unanchorable.
      const best = scored
        .filter((item) => item.score === scored[0].score)
        .map((item) => item.candidate)
        .sort((a, b) => a - b);
      selected =
        typeof position === "number"
          ? best.reduce((a, b) => (Math.abs(b - position) < Math.abs(a - position) ? b : a))
          : best[0];
    }
  }
  return { start: selected, end: selected + exact.length };
}

/**
 * Locate a comment's source selector in a tree of source files. The path it
 * was cut from is tried first, since that is where it almost always still
 * is; a file that has since been renamed has no text at that path any more,
 * so every other file in the tree is tried instead, and the selector is
 * accepted only when exactly one of them finds it -- the same rule
 * `sourcePlaceInTree` applies to a caret, for the same reason: a source
 * quote landing in the wrong file is worse than one that lands nowhere.
 * Returns {path, start, end}, or null.
 */
export function anchorSource(tree, source) {
  if (!source?.exact) return null;
  const texts = tree?.texts || {};
  const own = texts[source.path];
  if (own !== undefined) {
    const found = anchorOne(own, source, flatten(own));
    return found ? { path: source.path, start: found.start, end: found.end } : null;
  }

  let hit = null;
  for (const [path, text] of Object.entries(texts)) {
    const found = anchorOne(text, source, flatten(text));
    if (!found) continue;
    if (hit) return null; // found in more than one file: which one it meant is a guess
    hit = { path, start: found.start, end: found.end };
  }
  return hit;
}

/**
 * Re-anchor every comment's source selector against a tree, in one pass.
 * Mutates each comment that has a `source` with `sourceStart`, `sourceEnd`
 * and `sourcePath` -- null when the passage cannot be placed -- and counts
 * only over those comments, mirroring `anchorAll`.
 */
export function anchorAllSources(tree, comments) {
  let anchored = 0;
  let orphaned = 0;
  for (const comment of comments) {
    if (!comment.source) continue;
    const found = anchorSource(tree, comment.source);
    if (found) {
      comment.sourcePath = found.path;
      comment.sourceStart = found.start;
      comment.sourceEnd = found.end;
      anchored++;
    } else {
      comment.sourcePath = null;
      comment.sourceStart = null;
      comment.sourceEnd = null;
      orphaned++;
    }
  }
  return { anchored, orphaned };
}

/**
 * Re-anchor every comment against one document, in a single pass over the
 * already-extracted text. Mutates each comment with `start`, `end` and
 * `orphaned` -- all derived values that are never sent to the server.
 */
export function anchorAll(text, comments, view = null) {
  let anchored = 0;
  let orphaned = 0;
  // One flattened view for the whole pass, not one per comment. A caller
  // that already has one (e.g. reused across several anchorAll calls on the
  // same document) can pass it in instead of paying to rebuild it.
  view = view || flatten(text);
  for (const comment of comments) {
    const position = anchorOne(text, comment, view);
    if (position) {
      comment.start = position.start;
      comment.end = position.end;
      comment.orphaned = false;
      anchored++;
    } else {
      comment.start = comment.end = null;
      comment.orphaned = true;
      orphaned++;
    }
  }
  return { anchored, orphaned };
}
