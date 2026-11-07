// Find references (Shift-F12): lists every use of the label, citation or
// reference at the caret in a bottom panel. Each row opens its location.

import { StateEffect, StateField } from "@codemirror/state";
import { keymap, showPanel } from "@codemirror/view";
import { references as queryReferences } from "../document-index/queries.js";
import { m } from "../paraglide/messages.js";

/** @typedef {import("./context.js").IndexContext} IndexContext */
/** @typedef {import("../document-index/schema.js").Location} Location */
/** @typedef {import("@codemirror/state").EditorState} EditorState */
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
 * @param {EditorState} state the state of the file on screen
 * @param {IndexContext} context
 * @param {Location[]} locations
 * @returns {ReferenceRow[]}
 */
export function referenceRows(state, context, locations) {
  const onScreen = context.path();
  return locations.map(({ path, from }) => {
    if (path !== onScreen || from > state.doc.length) return { path, from, text: path };
    const line = state.doc.lineAt(from).number;
    return { path, from, text: m.references_location({ path, line }) };
  });
}

/** Opens the panel with these rows, or closes it with null. */
/** @type {import("@codemirror/state").StateEffectType<ReferenceRow[] | null>} */
const showReferences = StateEffect.define();

/**
 * @param {IndexContext} context
 * @param {ReferenceRow[]} rows
 * @param {() => void} close
 * @returns {Panel}
 */
function createPanel(context, rows, close) {
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
      first?.focus();
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
    const head = view.state.selection.main.head;
    const locations = queryReferences(context.project(), context.path(), head);
    if (locations.length === 0) return false;
    view.dispatch({ effects: showReferences.of(referenceRows(view.state, context, locations)) });
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
      for (const effect of tr.effects) {
        if (effect.is(showReferences)) value = effect.value;
      }
      return value;
    },
    provide: (f) =>
      showPanel.from(f, (rows) =>
        rows === null ? null : (view) => createPanel(context, rows, () => closePanel(view)),
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
