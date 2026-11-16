// What the document-index editor extensions need from the editor, and the
// signal that the project index has been refreshed.
//
// The extensions never read the session or the projection themselves. The
// editor hands them an IndexContext; they read the index through it and act
// through it, so every extension can be tested with a plain object.

import { StateEffect } from "@codemirror/state";

/** @typedef {import("../document-index/project.js").ProjectIndex} ProjectIndex */
/** @typedef {import("../document-index/schema.js").Edit} Edit */

/**
 * @typedef {{
 *   project: () => ProjectIndex,
 *   path: () => string,
 *   open: (path: string, at: number) => void,
 *   applyEdits: (edits: Edit[]) => void,
 * }} IndexContext
 *
 * - `project`: the current project index.
 * - `path`: the project path of the file on screen.
 * - `open`: show a project file and put the caret at a UTF-16 offset in it;
 *   for the file on screen it only moves the caret.
 * - `applyEdits`: apply edits across files, through tracked changes when they
 *   are on. Offsets are in each file's current text.
 */

/** Dispatched with `null` whenever the project index has been refreshed. */
export const indexChanged = StateEffect.define();
