// Rename (F2): a top panel that renames the label at the caret, and every
// reference to it, across the project.

import { StateEffect, StateField } from "@codemirror/state";
import { keymap, showPanel } from "@codemirror/view";
import { itemAt, rename as renameEdits, renameProblem } from "../document-index/queries.js";
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
  // Label and footnote refs are renamed with their definitions; a ref that is a citation is not.
  const namespace = item.kind === "ref" ? project.namespaceOf(item, context.path()) : null;
  if (namespace === "label" || namespace === "footnote") return item;
  return null;
}

/** Opens the panel for this target, or closes it with null. */
/** @type {import("@codemirror/state").StateEffectType<RenameTarget | null>} */
const showRename = StateEffect.define();

/**
 * @param {EditorView} view
 * @param {IndexContext} context
 * @param {() => RenameTarget} getTarget the target now, with `head` mapped through edits since the panel opened
 * @returns {Panel}
 */
function createPanel(view, context, getTarget) {
  const target = getTarget();
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
    const { head } = getTarget();
    const problem = renameProblem(context.project(), context.path(), head, input.value);
    const edits = problem === null ? renameEdits(context.project(), context.path(), head, input.value) : null;
    if (edits === null) {
      status.textContent =
        problem === "chars" ? m.rename_invalid_chars() : problem === "exists" ? m.rename_exists() : m.rename_invalid();
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
  /**
   * The open target and the panel constructor made for it. The constructor
   * stays the same while `head` is mapped, so edits do not rebuild the panel.
   * @typedef {RenameTarget & { make: (view: EditorView) => Panel }} OpenRename
   */
  /** @type {StateField<OpenRename | null>} */
  const field = StateField.define({
    create: () => null,
    update(value, tr) {
      if (value !== null && tr.docChanged) value = { ...value, head: tr.changes.mapPos(value.head) };
      for (const effect of tr.effects) {
        if (!effect.is(showRename)) continue;
        value =
          effect.value === null
            ? null
            : {
                ...effect.value,
                make: (view) => createPanel(view, context, () => /** @type {OpenRename} */ (view.state.field(field))),
              };
      }
      return value;
    },
    provide: (f) => showPanel.from(f, (target) => (target === null ? null : target.make)),
  });

  return [field, keymap.of([{ key: "F2", run: renameCommand(context), preventDefault: true }])];
}
