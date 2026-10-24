// The browser's spellcheck for a prose file. The content attribute turns it
// on for the whole editor, and a decoration sets spellcheck="false" on the
// markup spans inside it, which the browser inherits, so only prose is checked.
import { EditorView, ViewPlugin, Decoration } from "@codemirror/view";
import { RangeSetBuilder, StateEffect } from "@codemirror/state";
import { skipRanges } from "./index.js";

// Above this length a scan, a full pass over the text, costs more than the
// underlines are worth, so no markup is marked.
const MAX_SCAN_LENGTH = 2_000_000;

// An edit shifts the ranges already found, which is cheap, and the text is
// scanned again this many milliseconds after the last one.
const RESCAN_DELAY = 400;

const skipMark = Decoration.mark({ attributes: { spellcheck: "false" } });

// Dispatched after a rescan so that CodeMirror rebuilds the decorations.
const rescanned = StateEffect.define();

// One extension per format. The editor reconfigures its switches whenever one
// changes or a file is shown, and handing CodeMirror the same plugin keeps it
// rather than scanning the text again.
const extensions = new Map();

/**
 * The browser's spellcheck for a prose file, with its markup marked
 * spellcheck="false" so only the prose is checked.
 * @param {string} format
 * @returns {import("@codemirror/state").Extension}
 */
export function spellcheckExtension(format) {
  if (!extensions.has(format)) extensions.set(format, build(format));
  return extensions.get(format);
}

function build(format) {
  const markup = ViewPlugin.fromClass(class {
    constructor(view) {
      this.view = view;
      this.ranges = scanFor(view.state.doc);
      this.timer = null;
      this.decorations = this.build(view);
    }

    update(update) {
      if (update.docChanged) {
        // Assoc 1 at the start and -1 at the end, so text typed at either edge
        // of a range is not absorbed into it. The rescan decides that later.
        const changes = update.changes;
        this.ranges = this.ranges.flatMap(([from, to]) => {
          const start = changes.mapPos(from, 1);
          const end = changes.mapPos(to, -1);
          return end > start ? [/** @type {[number, number]} */ ([start, end])] : [];
        });
        clearTimeout(this.timer);
        this.timer = setTimeout(() => this.rescan(), RESCAN_DELAY);
      }
      const rescanDone = update.transactions.some((tr) => tr.effects.some((e) => e.is(rescanned)));
      if (update.docChanged || update.viewportChanged || rescanDone) {
        this.decorations = this.build(update.view);
      }
    }

    rescan() {
      this.timer = null;
      this.ranges = scanFor(this.view.state.doc);
      this.view.dispatch({ effects: rescanned.of(null) });
    }

    destroy() {
      clearTimeout(this.timer);
    }

    // Marks the part of each skipped range that is on screen. The ranges are
    // sorted and disjoint, and so are the visible ranges, so one pointer walks
    // both. A range can cover several visible ranges, hence the inner loop.
    build(view) {
      /** @type {RangeSetBuilder<Decoration>} */
      const builder = new RangeSetBuilder();
      const visible = view.visibleRanges;
      let v = 0;
      for (const [from, to] of this.ranges) {
        while (v < visible.length && visible[v].to <= from) v++;
        for (let j = v; j < visible.length && visible[j].from < to; j++) {
          const start = Math.max(from, visible[j].from);
          const end = Math.min(to, visible[j].to);
          if (end > start) builder.add(start, end, skipMark);
        }
      }
      return builder.finish();
    }
  }, {
    decorations: (plugin) => plugin.decorations,
  });

  function scanFor(doc) {
    if (doc.length > MAX_SCAN_LENGTH) return [];
    return skipRanges(doc.toString(), format);
  }

  return [EditorView.contentAttributes.of({ spellcheck: "true" }), markup];
}
