// Files with no preview of their own are not errors, so they are notes.
// They appear in Diagnostics but never count as errors or warnings.

/**
 * @typedef {{ severity: "warning" | "info", message: string, file?: string, line?: number, source: "preview" }} PreviewNote
 */

/**
 * Produce a note for the open file when the preview is not drawing it itself.
 * `target` is the resolved preview target: the file drawn and why. A file with
 * nothing to draw says so, and a file drawn inside its includer says where.
 * A locked preview says nothing, since the lock is shown by the pane itself.
 * @param {{ openPath: string | null, openIsText: boolean, target: { path: string | null, reason: string } }} options
 * @returns {PreviewNote[]}
 */
export function previewNotes({ openPath, openIsText, target }) {
  if (!openPath || !openIsText) return [];
  const name = openPath.split("/").pop();
  if (target.reason === "none") {
    return [{ severity: "info", message: `${name} has no preview of its own.`, file: openPath, line: 0, source: "preview" }];
  }
  if (target.reason === "inside" && target.path) {
    return [{ severity: "info", message: `${name} is shown inside ${target.path}.`, file: openPath, line: 0, source: "preview" }];
  }
  return [];
}
