// A cursor at a position of a LoroText, made to work on astral characters.
//
// Measured on loro-crdt 1.16.1 (the web build). Take a LoroText holding
// "café ☕ naïve" and insert an emoji at 4 as an edit of its own, as typing
// does. The emoji is one astral character at UTF-16 units 4 and 5:
//
//   text.getCursor(4, -1), (4, 1), (4, 0)  -> undefined
//   text.getCursor(5, 1)   -> a cursor; doc.getCursorPos gives {offset: 4, side: 1}
//   text.getCursor(5, -1)  -> a cursor; {offset: 4, side: -1}
//   text.getCursor(6, -1)  -> a cursor; {offset: 6, side: -1}
//   text.getCursor(3, 1)   -> a cursor; {offset: 3, side: 1}
//
// Loro answers no cursor at the FIRST unit of an astral character when that
// character begins an insertion placed before or inside text that was already
// there, and answers the cursor of that same character when given its SECOND
// unit. It does answer at the first unit when the character arrived inside a
// longer insertion ("ab😀cd" in one piece) or was appended at the very end.
// A commit in between makes no difference. Letters and BMP characters such as
// "é" are not affected.
//
// cursorAt asks Loro first. Only when Loro answers undefined and the character
// at `pos` is a surrogate pair does it ask again at pos + 1. Every other
// answer is Loro's own. If a later Loro answers a cursor for the first unit,
// the first call succeeds and the fallback is never reached, so it is harmless.
//
// This module imports nothing: it is loaded in plain Node tests and workers.

/**
 * @param {import("loro-crdt").LoroText} text
 * @param {number} pos UTF-16 offset
 * @param {-1 | 0 | 1} side
 * @param {string} [body] the text's current string, when the caller holds it
 */
export function cursorAt(text, pos, side, body) {
  const cursor = text.getCursor(pos, side);
  if (cursor !== undefined) return cursor;
  const s = body ?? text.toString();
  const hi = s.charCodeAt(pos);
  const lo = s.charCodeAt(pos + 1);
  if (hi >= 0xd800 && hi <= 0xdbff && lo >= 0xdc00 && lo <= 0xdfff) return text.getCursor(pos + 1, side);
  return cursor;
}
