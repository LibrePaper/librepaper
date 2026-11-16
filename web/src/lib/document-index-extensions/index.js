// The document-index editor extensions, in the two groups the editor places
// separately: editing aids that are always on, and diagnostics, which follow
// the editor's "check" setting like the compiler's.

import { navigation } from "./navigation.js";
import { indexHover } from "./hover.js";
import { references } from "./references.js";
import { rename } from "./rename.js";
import { indexDiagnostics } from "./index-diagnostics.js";

export { indexChanged } from "./context.js";
export { indexDiagnostics };

/** @typedef {import("./context.js").IndexContext} IndexContext */

/**
 * Go to definition, hover, find references and rename.
 * @param {IndexContext} context
 * @returns {import("@codemirror/state").Extension}
 */
export function indexEditing(context) {
  return [navigation(context), indexHover(context), references(context), rename(context)];
}
