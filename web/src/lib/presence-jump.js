// Where a collaborator is, for a click on their avatar. The session reads the
// presence record and their published caret; this decides which file to open
// and which range to show. It touches no editor and no CRDT, so it can be
// tested on plain values.

/**
 * @param {{
 *   file?: string,
 *   caret?: { file: string, anchor: number, head?: number } | null,
 * }} input
 *   `file`: the file the person said they are in (their presence record).
 *   `caret`: their caret as the session resolved it -- the file it is in and
 *   the UTF-16 offsets there -- or null when they have no caret anywhere.
 * @returns {{ file: string, from: number | null, to: number | null } | null}
 *   Null when nothing is known of a file. `from` and `to` are null when there
 *   is no caret to show, in which case the file is opened and nothing else.
 */
export function presenceJumpTarget({ file = "", caret = null } = {}) {
  // The caret names the file it is really in, which is more reliable than the
  // record the person last wrote, so it wins when both are known.
  if (caret?.file && Number.isFinite(caret.anchor)) {
    const anchor = Math.max(0, caret.anchor);
    const head = Number.isFinite(caret.head) ? Math.max(0, caret.head) : anchor;
    return { file: caret.file, from: Math.min(anchor, head), to: Math.max(anchor, head) };
  }
  return file ? { file, from: null, to: null } : null;
}
