// When a reader is moved to a newer published version, and how they are put
// back where they were reading.
//
// A document publishes itself 30s after its author stops typing, so a reader
// with a page open is almost always a version or two behind. The offer used to
// be the whole answer: a button saying a newer version exists, and nothing
// happening until it was pressed. That is the right behaviour in the middle of
// a sentence and the wrong one the rest of the time -- an unaccepted offer is
// not neutral, because a comment cannot be made against a version the server
// has moved past, so a reader who ignores the button eventually finds that
// selecting a passage and writing about it is refused.
//
// So the update is taken automatically when the reader is plainly not in the
// middle of anything, and held for as long as they are. What makes that safe
// is not the policy below -- it is that the page comes back scrolled to the
// words they were reading, which is the other half of this module.

/**
 * Whether a newer version may be applied without being asked for.
 *
 * Held only for work that a swap would destroy: a passage selected and waiting
 * for a verb, and a draft being typed. Both are about a passage of the page on
 * screen, and both are lost or silently retargeted if the page changes under
 * them. Everything else -- scrolling, reading, a panel open, a thread being
 * read -- survives a swap that keeps the reader's place, which is why none of
 * it holds the update back.
 */
export function mayApplyUpdate({ offered = false, selecting = false, composing = false } = {}) {
  if (!offered) return false;
  return !selecting && !composing;
}

/** How much of the page to remember the reader's place by. */
const BOOKMARK = 160;
/** And how much on either side of it, to tell two identical passages apart. */
const CONTEXT = 64;
/** Below this there is not enough to find again, so there is no bookmark. */
const ENOUGH = 12;

/**
 * What the reader was looking at, as a quotation that can be found again.
 *
 * The offset comes from the frame, and it is an offset into the text that
 * frame published -- which the next version of the document does not have.
 * Nothing about it survives a re-render: a paragraph added above moves every
 * character after it. The words do survive, so the words are what is kept, in
 * the same shape as any other quotation this reader locates, and `anchorOne`
 * finds them in the new page exactly as it finds a comment's passage.
 *
 * Returns null when there is nothing worth remembering -- the top of the
 * document, an empty page, a bookmark too short to identify anything -- and a
 * null bookmark simply means the new version opens where it opens.
 */
export function bookmarkFrom(text, offset) {
  if (typeof text !== "string" || !text) return null;
  const at = Math.min(Math.max(0, Math.floor(Number(offset)) || 0), text.length);
  // The top of the viewport cuts wherever it happens to fall, and half a word
  // is a worse thing to look for than the whole one: back up to where the word
  // began, then forward over the space that separates it from the last one.
  let start = at;
  while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
  while (start < text.length && /\s/.test(text[start])) start += 1;
  let end = Math.min(text.length, start + BOOKMARK);
  if (end < text.length) {
    const cut = text.lastIndexOf(" ", end);
    if (cut > start + ENOUGH) end = cut;
  }
  const exact = text.slice(start, end).trim();
  if (exact.length < ENOUGH) return null;
  return {
    exact,
    prefix: text.slice(Math.max(0, start - CONTEXT), start),
    suffix: text.slice(end, Math.min(text.length, end + CONTEXT)),
  };
}
