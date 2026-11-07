// Rename (F2): a top panel that renames the label at the caret, and every
// reference to it, across the project.

import { StateEffect, StateField } from "@codemirror/state";
import { keymap, showPanel } from "@codemirror/view";
import { itemAt, rename as renameEdits } from "../document-index/queries.js";
import { m } from "../paraglide/messages.js";

/** @typedef {import("./context.js").IndexContext} IndexContext */
/** @typedef {import("../document-index/schema.js").IndexItem} IndexItem */
/** @typedef {import("@codemirror/state").EditorState} EditorState */
/** @typedef {import("@codemirror/state").Extension} Extension */
/** @typedef {import("@codemirror/view").EditorView} EditorView */
/** @typedef {import("@codemirror/view").Panel} Panel */

/**
 * What the panel renames: the caret position at open time, and the current name.
 * @typedef {{ head: number, name: string }} RenameTarget
 */

/**
 * The item the caret is on, when it is a label, or a ref that resolves to a
 * label. Cites, and either-refs that resolve to a citation, are not renamed.
 * @param {EditorState} state
 * @param {IndexContext} context
 * @returns {IndexItem | null}
 */
export function renameTarget(state, context) {
  const project = context.project();
  const item = itemAt(project, context.path(), state.selection.main.head);
  if (!item || item.name === undefined) return null;
  if (item.kind === "label") return item;
  if (item.kind === "ref" && project.namespaceOf(item) === "label") return item;
  return null;
}

/** Opens the panel for this target, or closes it with null. */
const showRename = StateEffect.define();

/**
 * @param {EditorView} view
 * @param {IndexContext} context
 * @param {RenameTarget} target
 * @returns {Panel}
 */
function createPanel(view, context, target) {
  /** Closes the panel and returns focus to the editor. */
  const close = () => {
    view.dispatch({ effects: showRename.of(null) });
    view.focus();
  };

  const dom = document.createElement("div");
  dom.className = "cm-index-rename";
  dom.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    close();
  });

  const form = document.createElement("form");

  const status = document.createElement("span");
  status.setAttribute("role", "status");

  const label = document.createElement("label");
  label.textContent = m.rename_label({ name: target.name });

  const input = document.createElement("input");
  input.type = "text";
  input.value = target.name;
  input.addEventListener("input", () => {
    status.textContent = "";
    input.removeAttribute("aria-invalid");
  });
  label.append(input);
  form.append(label);

  const submit = document.createElement("button");
  submit.type = "submit";
  submit.textContent = m.common_rename();
  form.append(submit);

  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.textContent = m.common_cancel();
  cancel.addEventListener("click", close);
  form.append(cancel);
  form.append(status);

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const edits = renameEdits(context.project(), context.path(), target.head, input.value);
    if (edits === null) {
      status.textContent = m.rename_invalid();
      input.setAttribute("aria-invalid", "true");
      input.focus();
      return;
    }
    close();
    context.applyEdits(edits);
  });

  dom.append(form);

  return {
    dom,
    top: true,
    mount() {
      input.focus();
      input.select();
    },
  };
}

/**
 * Open the rename panel for the label at the caret. Returns false when the
 * caret is not on a label.
 * @param {IndexContext} context
 * @returns {(view: EditorView) => boolean}
 */
export function renameCommand(context) {
  return (view) => {
    const item = renameTarget(view.state, context);
    if (!item || item.name === undefined) return false;
    view.dispatch({ effects: showRename.of({ head: view.state.selection.main.head, name: item.name }) });
    return true;
  };
}

/**
 * The rename extension: the panel and F2.
 * @param {IndexContext} context
 * @returns {Extension}
 */
export function rename(context) {
  const field = StateField.define({
    create: () => null,
    update(value, tr) {
      for (const effect of tr.effects) {
        if (effect.is(showRename)) value = effect.value;
      }
      return value;
    },
    provide: (f) =>
      showPanel.from(f, (target) => (target === null ? null : (view) => createPanel(view, context, target))),
  });

  return [field, keymap.of([{ key: "F2", run: renameCommand(context), preventDefault: true }])];
}
