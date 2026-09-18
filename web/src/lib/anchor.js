// Locate a comment's display mark in the rendered page. Source identity and
// the current source attachment come from the server; these searches only
// decide where to paint the mark the reader sees.

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
  // A point has no selected words: the boundary between its surrounding
  // context locates it after edits, and the old offset breaks repeated ties.
  if (selector?.point === true) {
    const position = selector.position;
    if (!Number.isInteger(position) || position < 0) return null;
    const prefix = String(selector.prefix || "");
    const suffix = String(selector.suffix || "");
    if (prefix || suffix) {
      const context = search(text, { exact: prefix + suffix, position: position - prefix.length });
      if (!context) return null;
      const at = context.start + prefix.length;
      return { start: at, end: at };
    }
    return position <= text.length ? { start: position, end: position } : null;
  }
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
    requireUnique: selector.requireUnique === true,
  });
  if (!loose || loose.end === loose.start) return null;
  return { start: view.map[loose.start], end: view.map[loose.end - 1] + 1 };
}

function search(text, { exact, prefix = "", suffix = "", position = null, requireUnique = false }) {
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
      if (requireUnique) return null;
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
 * What a comment looked like on the page it was made on.
 *
 * The words and the words around them, as the render had them. This is the
 * selector the highlight is drawn from and the only thing here that is ever
 * matched against text: it is display evidence, and it is treated as such.
 * What the comment is *about* is its source range, which the server owns.
 */
export function shownSelector(comment) {
  const seen = comment?.presentation || {};
  const exact = String(seen.rendered_exact || "");
  const position = Number.isInteger(seen.rendered_position_utf16)
    ? seen.rendered_position_utf16
    : null;
  return {
    exact,
    prefix: String(seen.rendered_prefix || ""),
    suffix: String(seen.rendered_suffix || ""),
    position,
    // A note left between two words has no words of its own; the position and
    // the context on either side are the whole of it.
    point: !exact && position !== null,
  };
}

/**
 * Whether a comment is about the document as a whole rather than a passage.
 *
 * An editor is told so outright, in the anchor the server wrote. A reader is
 * never sent that anchor -- source identity is not a reader's to have -- but is
 * sent what the page had, and a remark about the whole document is the one
 * that quoted nothing and pointed nowhere. The two roads reach the same
 * answer, so the card reads the same either way instead of telling a reader
 * that a general remark's passage is missing from the page.
 */
export function aboutWholeDocument(comment) {
  const anchor = comment?.original_anchor;
  if (anchor) return anchor.kind === "document";
  const seen = comment?.presentation || {};
  return !seen.rendered_exact && !Number.isInteger(seen.rendered_position_utf16);
}

/**
 * Where each comment's passage is in the source, according to the server.
 *
 * Nothing is searched for here. The server resolves every comment against the
 * document's own history whenever the document changes, and sends the answer
 * with the comment; this only turns the file id in that answer into the path
 * this browser knows it by. A comment the server could not place gets nulls,
 * and its card says so rather than the editor guessing at it.
 */
export function placeSources(session, comments) {
  let anchored = 0;
  let orphaned = 0;
  for (const comment of comments) {
    const anchor = comment.original_anchor;
    const range = comment.attachment?.resolved_range_utf16;
    const status = comment.attachment?.status;
    const placed = status === "exact" || status === "modified";
    const fileId = anchor?.kind === "source_text" ? anchor.target?.file_id : null;
    const path = fileId ? session?.paths?.get(fileId) : null;
    if (!path || !placed || !Array.isArray(range)) {
      if (fileId) orphaned++;
      comment.sourcePath = null;
      comment.sourceStart = null;
      comment.sourceEnd = null;
      continue;
    }
    comment.sourcePath = path;
    comment.sourceStart = range[0];
    comment.sourceEnd = range[1];
    anchored++;
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
    const position = anchorOne(text, shownSelector(comment), view);
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
