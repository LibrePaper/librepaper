// Files with no renderer are not errors, so they are notes.
// They appear in Diagnostics but never count as errors or warnings.

/**
 * @typedef {{ severity: "warning" | "info", message: string, file?: string, line?: number, source: "preview" }} PreviewNote
 */

/**
 * The message shown when the main file cannot be rendered, or "" when it can.
 * @param {{ mainPath: string | null, formatOf: (path: string) => string | null }} options
 * @returns {string}
 */
export function unrenderableMain({ mainPath, formatOf }) {
  if (mainPath && !formatOf(mainPath)) {
    return `${mainPath} is not a format LibrePaper renders. Make a .tex, .typ, .qmd or .md file the main file.`;
  }
  return "";
}

/**
 * Produce diagnostics for files that have no preview of their own.
 * @param {{ openPath: string | null, openIsText: boolean, mainPath: string | null, formatOf: (path: string) => string | null }} options
 * @returns {PreviewNote[]}
 */
export function previewNotes({ openPath, openIsText, mainPath, formatOf }) {
  /** @type {PreviewNote[]} */
  const notes = [];

  // The warning says all there is to say: a note that the preview shows the
  // main file would point at a preview that does not exist.
  const mainMessage = unrenderableMain({ mainPath, formatOf });
  if (mainMessage) {
    return [{ severity: "warning", message: `Nothing to preview: ${mainMessage}`, source: "preview" }];
  }

  if (
    openPath &&
    openIsText &&
    !formatOf(openPath) &&
    openPath !== mainPath
  ) {
    notes.push({
      severity: "info",
      message: `This file has no preview of its own${mainPath ? `; the preview shows ${mainPath}` : ""}.`,
      file: openPath,
      line: 0,
      source: "preview",
    });
  }

  return notes;
}
