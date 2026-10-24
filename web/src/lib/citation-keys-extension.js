// Underlines citation keys that have no bibliography entry, and shows a card
// for the ones that do. The keys themselves come from citation-keys.js.
import { EditorView, Decoration, ViewPlugin, hoverTooltip } from "@codemirror/view";
import { StateEffect } from "@codemirror/state";
import { citedKeys, unknownCitations } from "./citation-keys.js";
import { entryLabel } from "./bibliography.js";

const MAX_SCAN_LENGTH = 2_000_000;
const RESCAN_DELAY = 400;

// Dispatched when the bibliography changes, so the underlines are drawn again.
export const citationsChanged = StateEffect.define();
// Dispatched after the delayed rescan, which then runs in the next update.
const rescanned = StateEffect.define();

const missing = Decoration.mark({ class: "cm-citation-missing", attributes: { title: "No bibliography entry for this key" } });

/**
 * Underlines unknown citation keys and shows a card for known ones. Each
 * argument is a getter, so the state of the editor is read when it is needed.
 * @param {{ entries: () => Array<object> | null, format: () => string, labels: () => Set<string> }} options
 * @returns {import("@codemirror/state").Extension[]}
 */
export function citationKeysExtension({ entries, format, labels }) {
  function scan(doc) {
    const list = entries();
    if (doc.length > MAX_SCAN_LENGTH || list == null) return [];
    return unknownCitations({ text: doc.toString(), format: format(), entries: list, labels: labels() })
      .map(({ from, to }) => [from, to]);
  }

  const underlines = ViewPlugin.fromClass(class {
    constructor(view) {
      this.view = view;
      this.timer = null;
      this.ranges = scan(view.state.doc);
      this.decorations = this.build();
    }

    update(update) {
      if (update.docChanged) {
        // Ranges follow the edit now, and the rescan corrects them after a pause.
        const changes = update.changes;
        this.ranges = this.ranges.flatMap(([from, to]) => {
          const start = changes.mapPos(from, 1);
          const end = changes.mapPos(to, -1);
          return end > start ? [/** @type {[number, number]} */ ([start, end])] : [];
        });
        clearTimeout(this.timer);
        this.timer = setTimeout(() => this.rescan(), RESCAN_DELAY);
      }
      const changed = update.transactions.some((tr) => tr.effects.some((e) => e.is(citationsChanged)));
      const rescanDone = update.transactions.some((tr) => tr.effects.some((e) => e.is(rescanned)));
      if (changed || rescanDone) {
        clearTimeout(this.timer);
        this.timer = null;
        this.ranges = scan(update.state.doc);
      }
      if (update.docChanged || changed || rescanDone) this.decorations = this.build();
    }

    rescan() {
      this.timer = null;
      this.view.dispatch({ effects: rescanned.of(null) });
    }

    destroy() {
      clearTimeout(this.timer);
    }

    build() {
      return Decoration.set(this.ranges.map(([from, to]) => missing.range(from, to)), true);
    }
  }, {
    decorations: (plugin) => plugin.decorations,
  });

  const hover = hoverTooltip((view, pos) => {
    const list = entries();
    if (list == null || view.state.doc.length > MAX_SCAN_LENGTH) return null;
    const hit = citedKeys(view.state.doc.toString(), format(), { labels: labels() })
      .find((cite) => cite.from <= pos && pos < cite.to);
    if (!hit) return null;
    const entry = list.find((item) => item.key === hit.key);
    return { pos: hit.from, end: hit.to, above: true, create: () => ({ dom: card(entry, hit.key) }) };
  });

  const theme = EditorView.baseTheme({
    ".cm-citation-missing": { textDecoration: "underline wavy var(--color-warning-solid, #e69f00)", textUnderlineOffset: "3px" },
    ".cm-citation-card": { padding: "0.4rem 0.6rem", maxWidth: "28rem" },
  });

  return [underlines, hover, theme];
}

function card(entry, key) {
  const dom = document.createElement("div");
  dom.className = "cm-citation-card";
  if (!entry) {
    dom.textContent = `No bibliography entry for "${key}"`;
    return dom;
  }
  const label = document.createElement("div");
  label.textContent = entryLabel(entry);
  dom.append(label);
  if (entry.doi) {
    const link = document.createElement("a");
    link.href = `https://doi.org/${entry.doi}`;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.textContent = entry.doi;
    dom.append(link);
  }
  return dom;
}
