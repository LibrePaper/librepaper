/// What a document is written in, which follows from what its main file is
/// called. A document is a directory, so the format is a property of one file
/// in it rather than of the whole.
export function formatOf(path) {
  const lower = (path || "").toLowerCase();
  if (lower.endsWith(".typ")) return "typst";
  if (lower.endsWith(".qmd")) return "quarto";
  if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
  if (lower.endsWith(".tex") || lower.endsWith(".ltx")) return "latex";
  return "";
}
