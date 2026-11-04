/// The language each source extension is written in. This is the one place
/// that says which extension is which language; add a language's extensions
/// here and every caller follows. `.ist` is deliberately absent: it is a
/// makeindex style file, not TeX syntax.
const LATEX_DOCUMENT_EXTENSIONS = [".tex", ".ltx"];
const SOURCE_EXTENSIONS = {
  latex: [
    ...LATEX_DOCUMENT_EXTENSIONS,
    ".sty", ".cls", ".def", ".clo", ".fd", ".cfg", ".bbx", ".cbx", ".lbx", ".lco",
    ".pgf", ".tikz", ".xmpdata",
  ],
  typst: [".typ"],
  quarto: [".qmd"],
  markdown: [".md", ".markdown"],
  html: [".html", ".htm"],
};

/// The language a source file is written in, from its extension. This answers
/// the question highlighting and completion need: a `.sty` is LaTeX source.
/// `formatOf` is the narrower question, which files are documents that render
/// on their own; a `.sty` is LaTeX but never a document.
export function sourceFormatOf(path) {
  const lower = (path || "").toLowerCase();
  for (const [format, extensions] of Object.entries(SOURCE_EXTENSIONS)) {
    if (extensions.some((extension) => lower.endsWith(extension))) return format;
  }
  return "";
}

/// What a document is written in, which follows from what its file is
/// called. A document is a directory, so the format is a property of one file
/// in it rather than of the whole. Only the formats in SOURCE_EXTENSIONS that
/// render on their own count, and of LaTeX only the top-level `.tex` and `.ltx`.
export function formatOf(path) {
  const format = sourceFormatOf(path);
  if (format !== "latex") return format;
  const lower = (path || "").toLowerCase();
  return LATEX_DOCUMENT_EXTENSIONS.some((extension) => lower.endsWith(extension)) ? "latex" : "";
}

/// What the preview shows a file as. A document format previews as itself.
/// Every other non-empty path previews as plain text. Every caller passes only
/// paths of text files already in the project (from `tree.texts`, `tree.main`,
/// or a file with `kind === "text"`), and the server's configurable
/// `text_extensions` already decided they are text. A second list here could
/// only drift, and it would miss extensions a deployment adds in its own
/// config. So the server's allowlist, not this function, decides which files
/// exist; an empty path is the only one with no preview.
export function previewFormatOf(path) {
  const format = formatOf(path);
  if (format) return format;
  return path ? "text" : "";
}
