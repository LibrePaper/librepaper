// The matches of the project search, marked in the source editor.
//
// The extension compiles the panel's options with the same matcher the results
// list uses, so the editor marks exactly what the panel finds: same case, whole
// word, regex and unicode rules. Only the lines in view are scanned, and the
// marks are rebuilt when the text or the viewport changes. Given no
// options, an empty query or an invalid pattern, it marks nothing.

import { Decoration, ViewPlugin } from "@codemirror/view";
import { compileQuery } from "./project-search.js";

const MARK_MAX = 2000;
const mark = Decoration.mark({ class: "cm-searchMatch" });

/**
 * @param {import("@codemirror/view").EditorView} view
 * @param {RegExp} re
 */
function build(view, re) {
  const { state } = view;
  const found = [];
  let last = -1;
  for (const range of view.visibleRanges) {
    // Whole lines, so a match that starts before the viewport edge is still seen.
    const start = Math.max(state.doc.lineAt(range.from).from, last);
    const end = state.doc.lineAt(range.to).to;
    if (end <= start) continue;
    last = end;
    const slice = state.sliceDoc(start, end);
    re.lastIndex = 0;
    let hit;
    while ((hit = re.exec(slice)) && found.length < MARK_MAX) {
      if (hit[0] === "") {
        // A zero-length match marks nothing; step past it so the loop ends.
        const code = slice.charCodeAt(re.lastIndex);
        re.lastIndex += re.unicode && code >= 0xd800 && code <= 0xdbff ? 2 : 1;
        continue;
      }
      found.push(mark.range(start + hit.index, start + hit.index + hit[0].length));
    }
    if (found.length >= MARK_MAX) break;
  }
  return Decoration.set(found, true);
}

/**
 * @param {{ query: string, caseSensitive?: boolean, wholeWord?: boolean, regex?: boolean } | null} options
 * @returns {import("@codemirror/state").Extension}
 */
export function searchMarks(options) {
  if (!options) return [];
  const { re } = compileQuery(options);
  if (!re) return [];
  return ViewPlugin.fromClass(
    class {
      constructor(view) {
        this.decorations = build(view, re);
      }
      update(update) {
        if (update.docChanged || update.viewportChanged) this.decorations = build(update.view, re);
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
