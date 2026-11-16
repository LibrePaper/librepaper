// Find references (Shift-F12): lists every use of the label, citation or
// reference at the caret in a bottom panel. Each row opens its location.

import { StateEffect, StateField } from "@codemirror/state";
import { keymap, showPanel } from "@codemirror/view";
import { referencesTo, targetAt } from "../document-index/queries.js";
import { m } from "../paraglide/messages.js";
import { indexChanged } from "./context.js";

/** @typedef {import("./context.js").IndexContext} IndexContext */
/** @typedef {import("../document-index/queries.js").Target} Target */
/** @typedef {import("../document-index/schema.js").Location} Location */
/** @typedef {import("@codemirror/state").Text} Text */
/** @typedef {import("@codemirror/state").Extension} Extension */
/** @typedef {import("@codemirror/view").EditorView} EditorView */
/** @typedef {import("@codemirror/view").Panel} Panel */

/**
 * One row of the panel.
 * @typedef {{ path: string, from: number, text: string }} ReferenceRow
 */

/**
 * The rows for a set of locations. Locations in the file on screen carry the
 * line number; locations in other files carry the path alone.
 * @param {Text} doc the text of the file on screen
 * @param {IndexContext} context
 * @param {Location[]} locations
 * @returns {ReferenceRow[]}
 */
export function referenceRows(doc, context, locations) {
  const onScreen = context.path();
  return locations.map(({ path, from }) => {
    if (path !== onScreen || from > doc.length) return { path, from, text: path };
    const line = doc.lineAt(from).number;
    return { path, from, text: m.references_location({ path, line }) };
  });
}

/**
 * What the panel shows: the symbol it was opened for, its rows, and whether
 * the panel takes focus when it is mounted (only when it is first opened, not
 * when a refreshed index recomputes the rows).
 * @typedef {{ target: Target, rows: ReferenceRow[], focus: boolean }} ReferencesPanel
 */

/** Opens the panel for this symbol, or closes it with null. */
/** @type {import("@codemirror/state").StateEffectType<ReferencesPanel | null>} */
const showReferences = StateEffect.define();

/**
 * The panel for a symbol, from the current index, or null when the symbol has
 * no references and no definition.
 * @param {Text} doc
 * @param {IndexContext} context
 * @param {Target} target
 * @param {boolean} focus
 * @returns {ReferencesPanel | null}
 */
function panelFor(doc, context, target, focus) {
  const locations = referencesTo(context.project(), target);
  if (locations.length === 0) return null;
  return { target, rows: referenceRows(doc, context, locations), focus };
}

/**
 * @param {IndexContext} context
 * @param {ReferenceRow[]} rows
 * @param {boolean} focus
 * @param {() => void} close
 * @returns {Panel}
 */
function createPanel(context, rows, focus, close) {
  const dom = document.createElement("div");
  dom.className = "cm-index-references";
  dom.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    close();
  });

  const header = document.createElement("div");
  header.className = "cm-index-references-header";

  const title = document.createElement("strong");
  title.textContent = m.references_title({ count: rows.length });
  header.append(title);

  const closeButton = document.createElement("button");
  closeButton.type = "button";
  closeButton.textContent = m.common_close();
  closeButton.addEventListener("click", close);
  header.append(closeButton);
  dom.append(header);

  const list = document.createElement("ul");
  /** @type {HTMLButtonElement | null} */
  let first = null;
  for (const row of rows) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = row.text;
    button.addEventListener("click", () => context.open(row.path, row.from));
    item.append(button);
    list.append(item);
    first ??= button;
  }
  dom.append(list);

  return {
    dom,
    mount() {
      if (focus) first?.focus();
    },
  };
}

/**
 * Find references to the item at the caret. Returns false, and opens nothing,
 * when there are none.
 * @param {IndexContext} context
 * @returns {(view: EditorView) => boolean}
 */
export function findReferencesCommand(context) {
  return (view) => {
    const target = targetAt(context.project(), context.path(), view.state.selection.main.head);
    if (!target) return false;
    const panel = panelFor(view.state.doc, context, target, true);
    if (!panel) return false;
    view.dispatch({ effects: showReferences.of(panel) });
    return true;
  };
}

/**
 * The find-references extension: the panel, Shift-F12, and Escape to close.
 * @param {IndexContext} context
 * @returns {Extension}
 */
export function references(context) {
  /** @param {EditorView} view */
  const closePanel = (view) => {
    view.dispatch({ effects: showReferences.of(null) });
    view.focus();
  };

  const field = StateField.define({
    create: () => null,
    update(value, tr) {
      let refresh = false;
      for (const effect of tr.effects) {
        if (effect.is(showReferences)) value = effect.value;
        if (effect.is(indexChanged)) refresh = true;
      }
      // The index was refreshed: recompute the open panel's rows for the same
      // symbol, from the same query as at open time. The panel closes when the
      // symbol has no references or definition left. Unchanged rows keep the
      // old panel, so an unrelated refresh does not rebuild it.
      if (refresh && value) {
        const next = panelFor(tr.newDoc, context, value.target, false);
        if (next === null || JSON.stringify(next.rows) !== JSON.stringify(value.rows)) value = next;
      }
      return value;
    },
    provide: (f) =>
      showPanel.from(f, (panel) =>
        panel === null
          ? null
          : (view) => createPanel(context, panel.rows, panel.focus, () => closePanel(view)),
      ),
  });

  return [
    field,
    keymap.of([
      { key: "Shift-F12", run: findReferencesCommand(context), preventDefault: true },
      {
        key: "Escape",
        run: (view) => {
          if (!view.state.field(field, false)) return false;
          closePanel(view);
          return true;
        },
      },
    ]),
  ];
}
