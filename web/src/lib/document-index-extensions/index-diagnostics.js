// Lint diagnostics from the document index: references to labels that are not
// defined, and includes or assets that name no project file. Each comes with
// one quick fix per suggestion. Unknown citations and unknown references are
// left to citation-keys-extension.js, which already underlines them.

import { linter } from "@codemirror/lint";
import { diagnostics } from "../document-index/queries.js";
import { indexChanged } from "./context.js";
import { m } from "../paraglide/messages.js";

/** @typedef {import("./context.js").IndexContext} IndexContext */
/** @typedef {import("@codemirror/lint").Diagnostic} LintDiagnostic */

/** The codes this extension reports. */
const REPORTED = new Set(["unknown-label", "missing-file"]);

/**
 * Lint diagnostics for the file on screen.
 * @param {IndexContext} context
 * @param {number} [docLength] the current length of the document; when given,
 *   ranges are clamped to it and ranges that start past it are dropped, since
 *   the index can lag the text by one refresh
 * @returns {import("@codemirror/lint").Diagnostic[]}
 */
export function indexDiagnosticsFor(context, docLength) {
  /** @type {LintDiagnostic[]} */
  const out = [];
  for (const diagnostic of diagnostics(context.project(), context.path())) {
    if (!REPORTED.has(diagnostic.code)) continue;
    let { from, to } = diagnostic;
    if (docLength !== undefined) {
      if (from >= docLength) continue;
      to = Math.min(to, docLength);
    }
    const { name, code, suggestions } = diagnostic;
    out.push({
      from,
      to,
      severity: "warning",
      source: "librepaper-index",
      message: code === "unknown-label"
        ? m.index_diagnostics_unknown_label({ name })
        : m.index_diagnostics_missing_file({ name }),
      actions: suggestions.map((suggestion) => ({
        name: m.index_diagnostics_replace({ name: suggestion }),
        apply(view, from, to) {
          view.dispatch({ changes: { from, to, insert: suggestion } });
        },
      })),
    });
  }
  return out;
}

/**
 * The lint source for the index diagnostics. It reruns when the index is
 * refreshed, as well as when the text changes.
 * @param {IndexContext} context
 * @returns {import("@codemirror/state").Extension}
 */
export function indexDiagnostics(context) {
  return linter((view) => indexDiagnosticsFor(context, view.state.doc.length), {
    delay: 400,
    needsRefresh: (update) =>
      update.transactions.some((tr) => tr.effects.some((effect) => effect.is(indexChanged))),
  });
}
