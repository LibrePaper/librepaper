// Lint diagnostics from the document index: references to labels or footnotes
// that are not defined, duplicate labels, includes, assets and file links that
// name no project file, and the unused notes (info). Each comes with one quick
// fix per suggestion. Unknown citations and unknown references are left to
// citation-keys-extension.js, which already underlines them.

import { linter } from "@codemirror/lint";
import { diagnostics } from "../document-index/queries.js";
import { indexChanged } from "./context.js";
import { m } from "../paraglide/messages.js";

/** @typedef {import("./context.js").IndexContext} IndexContext */
/** @typedef {import("@codemirror/lint").Diagnostic} LintDiagnostic */

/** The codes this extension reports. */
const REPORTED = new Set([
  "unknown-label",
  "unknown-footnote",
  "duplicate-label",
  "missing-file",
  "unused-footnote",
  "unused-target",
  "unused-bibliography",
]);

/**
 * The message for a diagnostic, through Paraglide.
 * @param {import("../document-index/schema.js").Diagnostic} diagnostic
 * @returns {string}
 */
function messageOf({ code, name }) {
  switch (code) {
    case "unknown-label":
      return m.index_diagnostics_unknown_label({ name });
    case "unknown-footnote":
      return m.index_diagnostics_unknown_footnote({ name });
    case "duplicate-label":
      return m.index_diagnostics_duplicate_label({ name });
    case "unused-footnote":
      return m.index_diagnostics_unused_footnote({ name });
    case "unused-target":
      return m.index_diagnostics_unused_target({ name });
    case "unused-bibliography":
      return m.index_diagnostics_unused_bibliography();
    default:
      return m.index_diagnostics_missing_file({ name });
  }
}

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
    const { suggestions } = diagnostic;
    out.push({
      from,
      to,
      severity: diagnostic.severity === "info" ? "info" : "warning",
      source: "librepaper-index",
      message: messageOf(diagnostic),
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
