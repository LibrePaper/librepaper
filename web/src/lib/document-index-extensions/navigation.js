// Go to definition: F12, and Mod-click on a reference, citation, include or asset.
// Mod is Cmd on macOS and Ctrl elsewhere. While Mod is held, the item under the
// mouse is underlined, so the click's target is visible before it is made.

import { StateEffect } from "@codemirror/state";
import { EditorView, Decoration, ViewPlugin, keymap } from "@codemirror/view";
import { definition, itemAt } from "../document-index/queries.js";

/** @typedef {import("@codemirror/state").Extension} Extension */
/** @typedef {import("@codemirror/view").ViewUpdate} ViewUpdate */
/** @typedef {import("./context.js").IndexContext} IndexContext */
/** @typedef {[number, number] | null} Span */

/** @param {KeyboardEvent | MouseEvent} event @returns {boolean} whether the Mod key is down */
function modDown(event) {
  const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  return mac ? event.metaKey : event.ctrlKey;
}

/**
 * Open the definition of the reference, citation, include or asset at the caret.
 * Does nothing, and returns false, when there is no definition.
 * @param {EditorView} view
 * @param {IndexContext} context
 * @returns {boolean}
 */
export function goToDefinition(view, context) {
  const [first] = definition(context.project(), context.path(), view.state.selection.main.head);
  if (!first) return false;
  context.open(first.path, first.from);
  return true;
}

const linkMark = Decoration.mark({ class: "cm-index-link" });

/** Sets the span to underline (a [from, to] pair), or null to clear it. */
const setLink = StateEffect.define();

/**
 * @param {IndexContext} context
 * @returns {Extension}
 */
export function navigation(context) {
  const linkPlugin = ViewPlugin.fromClass(
    class {
      /** @param {EditorView} view */
      constructor(view) {
        /** @type {EditorView} */
        this.view = view;
        /** @type {{ x: number, y: number } | null} */
        this.mouse = null;
        /** @type {Span} */
        this.current = null;
        this.decorations = Decoration.none;

        this.onMove = (/** @type {MouseEvent} */ event) => {
          this.mouse = { x: event.clientX, y: event.clientY };
          this.refresh(modDown(event));
        };
        this.onLeave = () => {
          this.mouse = null;
          this.refresh(false);
        };
        this.onKey = (/** @type {KeyboardEvent} */ event) => {
          if (event.key === "Control" || event.key === "Meta") {
            this.refresh(event.type === "keydown" && modDown(event));
          }
        };

        view.dom.addEventListener("mousemove", this.onMove);
        view.dom.addEventListener("mouseleave", this.onLeave);
        window.addEventListener("keydown", this.onKey);
        window.addEventListener("keyup", this.onKey);
      }

      /**
       * Underline the item under the mouse when the Mod key is down; clear otherwise.
       * @param {boolean} down
       */
      refresh(down) {
        const span = down && this.mouse ? this.spanUnderMouse() : null;
        if (sameSpan(span, this.current)) return;
        this.current = span;
        this.view.dispatch({ effects: setLink.of(span) });
      }

      /** @returns {Span} */
      spanUnderMouse() {
        const pos = this.view.posAtCoords(this.mouse);
        if (pos == null) return null;
        const project = context.project();
        const path = context.path();
        if (definition(project, path, pos).length === 0) return null;
        const item = itemAt(project, path, pos);
        if (!item) return null;
        return item.nameRange ?? item.range;
      }

      /** @param {ViewUpdate} update */
      update(update) {
        for (const transaction of update.transactions) {
          for (const effect of transaction.effects) {
            if (!effect.is(setLink)) continue;
            this.decorations = effect.value
              ? Decoration.set([linkMark.range(effect.value[0], effect.value[1])])
              : Decoration.none;
          }
        }
      }

      destroy() {
        this.view.dom.removeEventListener("mousemove", this.onMove);
        this.view.dom.removeEventListener("mouseleave", this.onLeave);
        window.removeEventListener("keydown", this.onKey);
        window.removeEventListener("keyup", this.onKey);
      }
    },
    {
      decorations: (plugin) => plugin.decorations,
    },
  );

  return [
    keymap.of([{ key: "F12", run: (view) => goToDefinition(view, context) }]),
    EditorView.domEventHandlers({
      /**
       * @param {MouseEvent} event
       * @param {EditorView} view
       */
      mousedown(event, view) {
        if (event.button !== 0 || !modDown(event)) return false;
        const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
        if (pos == null) return false;
        const [first] = definition(context.project(), context.path(), pos);
        if (!first) return false;
        event.preventDefault();
        context.open(first.path, first.from);
        return true;
      },
    }),
    linkPlugin,
    EditorView.baseTheme({
      ".cm-index-link": { textDecoration: "underline", cursor: "pointer" },
    }),
  ];
}

/**
 * @param {Span} a
 * @param {Span} b
 * @returns {boolean}
 */
function sameSpan(a, b) {
  if (a === null || b === null) return a === b;
  return a[0] === b[0] && a[1] === b[1];
}
