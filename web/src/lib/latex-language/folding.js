// Section folding for LaTeX, read from the syntax tree. Environments fold
// through foldNodeProp in index.js; this covers sectioning commands.
//
// A sectioning command (\part ... \subparagraph, optionally starred) that
// starts a line folds from the end of that line to the end of the line before
// the next sectioning command of the same or a higher level, the \end{document}
// that closes the document, or the end of the document. The tree decides what
// counts: a heading inside a verbatim body is one opaque token, so it never
// starts or ends a fold.

import { foldService, syntaxTree } from "@codemirror/language";

const SECTION_DEPTH = {
  part: 1,
  chapter: 2,
  section: 3,
  subsection: 4,
  subsubsection: 5,
  paragraph: 6,
  subparagraph: 7,
};

/** The sectioning depth of a SectionName node, read from its text (\section or \section*). */
function sectionDepth(state, node) {
  return SECTION_DEPTH[state.sliceDoc(node.from + 1, node.to).replace(/\*$/, "")];
}

/** Whether only whitespace comes before pos on its line. */
function startsLine(state, pos) {
  return /^\s*$/.test(state.sliceDoc(state.doc.lineAt(pos).from, pos));
}

/** The SectionName node that starts the line, after any indentation, or null. */
function headingOn(state, line) {
  const at = line.from + (line.text.length - line.text.trimStart().length);
  for (let node = syntaxTree(state).resolveInner(at, 1); node; node = node.parent) {
    if (node.name === "SectionName" && node.from === at) return node;
  }
  return null;
}

/**
 * The start of the first later heading of the same or higher level, or of
 * \end{document}, in document order. Null when neither follows.
 */
function sectionStop(state, heading) {
  const depth = sectionDepth(state, heading);
  const cursor = syntaxTree(state).cursorAt(heading.to, 1);
  do {
    if (cursor.from <= heading.from) continue;
    if (cursor.name === "SectionName") {
      if (startsLine(state, cursor.from) && sectionDepth(state, cursor.node) <= depth) return cursor.from;
    } else if (cursor.name === "EndEnv") {
      const name = cursor.node.getChild("EnvName");
      if (name && state.sliceDoc(name.from, name.to).trim() === "document") return cursor.from;
    }
  } while (cursor.next());
  return null;
}

/** Fold range for a line that starts with a sectioning command, or null. */
const sectionFolding = foldService.of((state, lineStart) => {
  const line = state.doc.lineAt(lineStart);
  // The gutter asks about every visible line; only lines with a heading need a tree walk.
  if (!/\\(part|chapter|section|subsection|subsubsection|paragraph|subparagraph)/.test(line.text)) return null;
  const heading = headingOn(state, line);
  if (!heading) return null;

  const stop = sectionStop(state, heading);
  // The fold ends on the line before the stop; with no stop it runs to the last line.
  const endLine = stop === null ? state.doc.lines : state.doc.lineAt(stop).number - 1;
  // An empty or single-line body has nothing worth folding.
  if (endLine <= line.number) return null;
  return { from: line.to, to: state.doc.line(endLine).to };
});

export { sectionFolding };
