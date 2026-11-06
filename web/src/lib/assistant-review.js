export function diagnosticContext(item, tree, revision) {
  const file = item.file || tree.main || "";
  const line = Number.isInteger(item.line) && item.line > 0 ? item.line : 0;
  return { ...item, file, line, revision,
    source: line ? (tree.texts?.[file] || "").split(/\r?\n/)[line - 1] || "" : "" };
}
