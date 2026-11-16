// Callers map this over every diagnostic of the same text, so the last split
// is reused while the text stays identical.
let splitText = null;
let splitLines = [];

function linesOf(text) {
  if (text !== splitText) {
    splitText = text;
    splitLines = text.split(/\r?\n/);
  }
  return splitLines;
}

export function diagnosticContext(item, tree, revision) {
  const file = item.file || tree.main || "";
  const line = Number.isInteger(item.line) && item.line > 0 ? item.line : 0;
  return { ...item, file, line, revision,
    source: line ? linesOf(tree.texts?.[file] || "")[line - 1] || "" : "" };
}
