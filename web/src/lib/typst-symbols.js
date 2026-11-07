// Typst symbols from the typst module. The module parses a file and answers its
// headings and top-level `#let` bindings as UTF-8 byte offsets (`depth` is the
// heading level, 1 for `=`). This module converts them to document-index items
// in UTF-16, and decides which outline the Outline panel shows.

import { ViewPlugin } from "@codemirror/view";
import { byteToUtf16, defaultRequest, folder } from "./typst-folding.js";
import { extractOutline } from "./outline.js";

const DELAY = 400;

/** The index kinds the module's answer replaces in the adapter's items. */
export const SYMBOL_KINDS = new Set(["heading", "definition"]);

/** A heading's name with the trailing ` <label>` the module leaves in it removed. */
export function stripLabel(name) {
  return String(name ?? "").replace(/\s*<[^<>\s]+>\s*$/u, "").trim();
}

/**
 * The module's answer as index items: `heading` (level, title) and
 * `definition` (name, nameRange, attrs.type) in UTF-16, in source order.
 * Labels are left to the adapter.
 * @param {string} source
 * @param {any} answer
 * @returns {import("./document-index/schema.js").IndexItem[]}
 */
export function symbolItems(source, answer) {
  const symbols = answer && Array.isArray(answer.symbols) ? answer.symbols : [];
  const convert = byteToUtf16(source);
  const items = [];
  for (const symbol of symbols) {
    if (!symbol || !Number.isFinite(symbol.from) || !Number.isFinite(symbol.to)) continue;
    const from = convert(symbol.from);
    const to = convert(symbol.to);
    if (symbol.kind === "heading") {
      if (!Number.isFinite(symbol.depth)) continue;
      items.push({ kind: "heading", range: [from, Math.max(from, to)], level: symbol.depth, title: stripLabel(symbol.name) });
    } else if (symbol.kind === "function" || symbol.kind === "variable") {
      if (!Number.isFinite(symbol.name_from) || !Number.isFinite(symbol.name_to) || typeof symbol.name !== "string") continue;
      const nameFrom = convert(symbol.name_from);
      const nameTo = convert(symbol.name_to);
      // The adapter's definition starts at the `#` of `#let`.
      const start = source[from - 1] === "#" ? from - 1 : from;
      items.push({
        kind: "definition",
        range: [start, nameTo],
        name: symbol.name,
        nameRange: [nameFrom, nameTo],
        attrs: { type: symbol.kind },
      });
    }
  }
  return items.sort((a, b) => a.range[0] - b.range[0]);
}

/**
 * Outline entries (`{title, level, from, line}`, as extractOutline returns)
 * from the heading items of `symbolItems`.
 * @param {string} source
 * @param {import("./document-index/schema.js").IndexItem[]} items
 */
export function headingsOf(source, items) {
  const out = [];
  let line = 1;
  let at = 0;
  for (const item of items) {
    if (item.kind !== "heading") continue;
    const from = item.range[0];
    for (let i = source.indexOf("\n", at); i !== -1 && i < from; i = source.indexOf("\n", at)) {
      line += 1;
      at = i + 1;
    }
    out.push({ title: item.title ?? "", level: item.level ?? 1, from, line });
  }
  return out;
}

/**
 * The Typst outline for `source`: the module's headings when `stored`
 * (`{source, headings}`) was computed from exactly this text, otherwise the
 * scanner's. A stale answer is never used, since its offsets would be wrong.
 */
export function typstOutline(source, stored) {
  if (stored && stored.source === source) return stored.headings;
  return extractOutline(source, "typst");
}

/** The default request: the module's `symbols` operation. */
export const symbolsRequest = (path, source) => defaultRequest(path, source, "symbols");

/**
 * A view plugin that asks for the file's symbols after edits, like folding.
 * `onSymbols(source, items)` gets each answer for the text it was computed
 * from; `onSymbols(null, null)` is called when the plugin is destroyed.
 */
export function typstSymbols({ onSymbols, request = symbolsRequest, path = () => "main.typ", delay = DELAY }) {
  return ViewPlugin.define((view) => {
    const job = folder({
      getDoc: () => view.state.doc,
      apply: (result) => { if (result) onSymbols(result.source, result.items); },
      convert: (source, answer) => (answer && Array.isArray(answer.symbols) ? { source, items: symbolItems(source, answer) } : null),
      request,
      path,
      delay,
    });
    job.schedule();
    return {
      update(update) { if (update.docChanged) job.schedule(); },
      destroy() {
        job.destroy();
        onSymbols(null, null);
      },
    };
  });
}
