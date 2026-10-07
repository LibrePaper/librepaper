// Files with no renderer are not errors, so they are notes.
// They appear in Diagnostics but never count as errors or warnings.

/**
 * @typedef {{ severity: "warning" | "info", message: string, file?: string, line?: number, source: "preview" }} PreviewNote
 */

/**
 * Produce diagnostics for files that have no preview of their own.
 * @param {{ openPath: string | null, openIsText: boolean, mainPath: string | null, formatOf: (path: string) => string | null }} options
 * @returns {PreviewNote[]}
 */
export function previewNotes({ openPath, openIsText, mainPath, formatOf }) {
  const notes = [];

  // If main file is set and cannot be rendered, warn about it.
  if (mainPath && !formatOf(mainPath)) {
    notes.push({
      severity: "warning",
      message: `Nothing to preview: ${mainPath} is not a format LibrePaper renders. Make a .tex, .typ, .qmd or .md file the main file.`,
      source: "preview",
    });
  }

  // If open file is text but not renderable, note that it shows the main file's preview.
  if (
    openPath &&
    openIsText &&
    !formatOf(openPath) &&
    openPath !== mainPath
  ) {
    const ext = openPath.split(".").pop()?.toLowerCase();
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
    const label = labels[ext] || "This file";
    const isPlural = label === "This file" ? false : true;
    const noun = isPlural ? "files" : "file";
    const verb = isPlural ? "have" : "has";
    const possessive = isPlural ? "their" : "its";
    let message;
    if (mainPath) {
      message = `${label} ${noun} ${verb} no preview of ${possessive} own; the preview shows ${mainPath}.`;
    } else {
      message = `${label} ${noun} ${verb} no preview of ${possessive} own.`;
    }
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
