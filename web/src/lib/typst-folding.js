// Typst code folding. The typst module parses the file and answers its
// foldable ranges as UTF-8 byte offsets; this module converts them to
// CodeMirror's UTF-16 positions, keeps them mapped through edits, and refreshes
// them a moment after typing stops.
//
// The module's ranges already have CodeMirror's shape: a region or comment
// runs from just after the opening delimiter to just before the closing one,
// and a heading section runs from the end of the heading line to the end of
// its last line of content. Hiding [from, to) therefore leaves the first line
// and the closing delimiter (or the next heading) visible.

import { StateEffect, StateField } from "@codemirror/state";
import { ViewPlugin } from "@codemirror/view";
import { foldService } from "@codemirror/language";
import { rendererRequest } from "./renderer-client.js";

const DELAY = 400;

/**
 * A function mapping a UTF-8 byte offset of `source` to a UTF-16 index. An
 * offset inside a character rounds down to its start; one past the end clamps.
 */
export function byteToUtf16(source) {
  const bytes = [];
  const units = [];
  let total = 0;
  for (let i = 0; i < source.length; ) {
    const code = source.codePointAt(i);
    bytes.push(total);
    units.push(i);
    total += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    i += code > 0xffff ? 2 : 1;
  }
  return (offset) => {
    if (!(offset > 0)) return 0;
    if (offset >= total) return source.length;
    let low = 0;
    let high = bytes.length - 1;
    while (low < high) {
      const mid = (low + high + 1) >> 1;
      if (bytes[mid] <= offset) low = mid;
      else high = mid - 1;
    }
    return units[low];
  };
}

/** The module's answer as sorted UTF-16 `{from, to}` ranges that span lines. */
export function foldRangesFrom(source, answer) {
  const ranges = answer && Array.isArray(answer.ranges) ? answer.ranges : [];
  const convert = byteToUtf16(source);
  const out = [];
  for (const range of ranges) {
    if (!range || !Number.isFinite(range.from) || !Number.isFinite(range.to)) continue;
    const from = convert(range.from);
    const to = convert(range.to);
    if (to <= from || !source.slice(from, to).includes("\n")) continue;
    out.push({ from, to });
  }
  return out.sort((a, b) => a.from - b.from || b.to - a.to);
}

/** The default request: the typst module, asked over the renderer worker. */
export function defaultRequest(path, source) {
  const modules = globalThis.LIBREPAPER_MODULES || {};
  const url = modules.typst;
  if (!url) return Promise.resolve(null);
  return rendererRequest(new URL(url, globalThis.location.href).href, "folding", { path, source });
}

export const setFolds = StateEffect.define();

/** The current fold ranges, mapped through every edit. */
export const foldsField = StateField.define({
  create: () => [],
  update(ranges, tr) {
    for (const effect of tr.effects) if (effect.is(setFolds)) return effect.value;
    if (!tr.docChanged || !ranges.length) return ranges;
    const mapped = [];
    for (const range of ranges) {
      const from = tr.changes.mapPos(range.from, 1);
      const to = tr.changes.mapPos(range.to, -1);
      if (to > from) mapped.push({ from, to });
    }
    return mapped;
  },
});

/** The fold range that starts on the line [lineStart, lineEnd], largest first. */
export function foldAt(ranges, lineStart, lineEnd) {
  let best = null;
  for (const range of ranges) {
    if (range.from < lineStart || range.from > lineEnd) continue;
    if (!best || range.to > best.to) best = range;
  }
  return best ? { from: best.from, to: best.to } : null;
}

/**
 * Refreshes folds: `schedule()` after a change waits, asks for the current
 * text, and applies the answer only if the document is still the one asked
 * about; otherwise it asks again. Returns `{schedule, destroy}`.
 */
export function folder({ getDoc, apply, request, path, delay = DELAY, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let timer = null;
  let dead = false;
  const run = async () => {
    timer = null;
    const doc = getDoc();
    let answer = null;
    try {
      answer = await request(path(), doc.toString());
    } catch {
      return;
    }
    if (dead) return;
    if (getDoc() !== doc) return schedule();
    apply(foldRangesFrom(doc.toString(), answer));
  };
  const schedule = () => {
    if (dead) return;
    if (timer !== null) clearTimer(timer);
    timer = setTimer(run, delay);
  };
  const destroy = () => {
    dead = true;
    if (timer !== null) clearTimer(timer);
    timer = null;
  };
  return { schedule, destroy };
}

/**
 * Typst folding. `request(path, source)` resolves to the module's answer;
 * `path` names the file in the request (the module only parses).
 */
export function typstFolding({ request = defaultRequest, path = () => "main.typ", delay = DELAY } = {}) {
  const plugin = ViewPlugin.define((view) => {
    const job = folder({
      getDoc: () => view.state.doc,
      apply: (ranges) => view.dispatch({ effects: setFolds.of(ranges) }),
      request,
      path,
      delay,
    });
    job.schedule();
    return {
      update(update) { if (update.docChanged) job.schedule(); },
      destroy() { job.destroy(); },
    };
  });
  return [
    foldsField,
    plugin,
    foldService.of((state, lineStart, lineEnd) => foldAt(state.field(foldsField), lineStart, lineEnd)),
  ];
}
