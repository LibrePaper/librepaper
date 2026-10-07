// Files with no renderer are not errors, so they are notes.
// They appear in Diagnostics but never count as errors or warnings.

/**
 * @typedef {{ severity: "warning" | "info", message: string, file?: string, line?: number, source: "preview" }} PreviewNote
 */

const labels = {
  bib: "BibTeX",
  json: "JSON",
  csv: "CSV",
  yml: "YAML",
  yaml: "YAML",
  cls: "LaTeX class and style",
  sty: "LaTeX class and style",
  lua: "Lua",
  txt: "Plain text",
};

/**
 * Produce diagnostics for files that have no preview of their own.
 * @param {{ openPath: string | null, openIsText: boolean, mainPath: string | null, formatOf: (path: string) => string | null }} options
 * @returns {PreviewNote[]}
 */
export function previewNotes({ openPath, openIsText, mainPath, formatOf }) {
  const notes = [];

  if (mainPath && !formatOf(mainPath)) {
    notes.push({
      severity: "warning",
      message: `Nothing to preview: ${mainPath} is not a format LibrePaper renders. Make a .tex, .typ, .qmd or .md file the main file.`,
      source: "preview",
    });
  }

  if (
    openPath &&
    openIsText &&
    !formatOf(openPath) &&
    openPath !== mainPath
  ) {
    const ext = openPath.split(".").pop()?.toLowerCase();
    const label = labels[ext] || "This file";
    const isPlural = label !== "This file";
    const message = isPlural
      ? `${label} files have no preview of their own${mainPath ? `; the preview shows ${mainPath}` : ""}.`
      : `This file has no preview of its own${mainPath ? `; the preview shows ${mainPath}` : ""}.`;
    notes.push({
      severity: "info",
      message,
      file: openPath,
      line: 0,
      source: "preview",
    });
  }

  return notes;
}
