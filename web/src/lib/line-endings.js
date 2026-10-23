// A file body in a document never holds a carriage return (gh#74).
//
// CodeMirror splits a CRLF into one line break, while the LoroText keeps both
// characters. The binding copies view offsets straight into the text, so a
// body with CRLF shifts every edit made on it. Bodies are therefore turned to
// LF where they enter a document, and a body that still holds a CR is
// rewritten before an editor binds it.

/// The origin of the commit that rewrites a body, which undo leaves alone.
export const LINE_ENDINGS_ORIGIN = "line-endings";

/// CRLF and a lone CR both become LF.
/** @param {string} text */
export function toLf(text) {
  return text.includes("\r") ? text.replace(/\r\n?/g, "\n") : text;
}

/// Rewrites the text to LF by a diff, so cursors on the kept characters keep
/// pointing at them. Returns whether the text changed.
/** @param {import("loro-crdt").LoroText} text @returns {boolean} */
export function healLineEndings(text) {
  const current = text.toString();
  if (!current.includes("\r")) return false;
  text.update(toLf(current));
  return true;
}
