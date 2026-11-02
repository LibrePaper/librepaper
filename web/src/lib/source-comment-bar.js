// The bar over a selection in the code pane.
//
// The preview has a bar over the words it selected; the code pane has the
// same verbs over the source range it selected, so one gesture makes the
// same annotation from either side. The bar is a CodeMirror cursor tooltip
// anchored at the selection head, shown only while there is a selection to
// act on and the caller says the reader may annotate.
import { StateField } from "@codemirror/state";
import { EditorView, showTooltip } from "@codemirror/view";

/**
 * A tooltip offering `verbs` over a non-empty selection.
 *
 * `enabled` and `verbs` are read on every transaction, so the caller can
 * change them without reconfiguring the editor. `onverb` gets the id of the
 * verb pressed; the caller reads the selection itself.
 *
 * @param {{
 *   enabled: () => boolean,
 *   verbs: () => Array<{ id: string, label: string, title: string }>,
 *   onverb: (id: string) => void,
 * }} options
 * @returns {import("@codemirror/state").Extension}
 */
export function sourceCommentBar({ enabled, verbs, onverb }) {
  // One create function for the life of the extension. CodeMirror reuses a
  // tooltip's DOM while its create function is the same object, so the bar
  // is built once per appearance rather than once per transaction.
  const create = () => {
    const dom = document.createElement("div");
    dom.className = "cm-source-comment-bar";
    dom.setAttribute("role", "toolbar");
    // Pressing a verb must not take the selection away from the editor.
    dom.addEventListener("mousedown", (event) => event.preventDefault());
    for (const verb of verbs()) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = verb.label;
      button.title = verb.title;
      button.dataset.verb = verb.id;
      button.addEventListener("click", () => onverb(verb.id));
      dom.append(button);
    }
    return { dom };
  };

  // The tooltip for a state, or null when there is nothing to offer.
  const barAt = (state) => {
    if (!enabled()) return null;
    const { from, to, head } = state.selection.main;
    if (from === to) return null;
    return { pos: head, above: head === from, strictSide: false, arrow: false, create };
  };

  const field = StateField.define({
    create: (state) => barAt(state),
    update: (_value, transaction) => barAt(transaction.state),
    provide: (f) => showTooltip.from(f),
  });

  const theme = EditorView.baseTheme({
    ".cm-source-comment-bar": {
      display: "flex",
      gap: "2px",
      padding: "2px",
      border: "1px solid var(--color-border)",
      borderRadius: "var(--radius-base)",
      background: "var(--color-raised)",
      boxShadow: "var(--shadow-lg)",
    },
    ".cm-source-comment-bar button": {
      font: "inherit",
      fontSize: "var(--text-sm)",
      whiteSpace: "nowrap",
      padding: "0.25rem 0.5rem",
      border: "0",
      borderRadius: "calc(var(--radius-base) - 2px)",
      background: "none",
      color: "var(--color-text-secondary)",
      cursor: "pointer",
    },
    ".cm-source-comment-bar button:hover": {
      background: "var(--color-row-hover)",
      color: "var(--color-link)",
    },
    // The tooltip's own frame would draw a second box around the bar.
    "& .cm-tooltip:has(> .cm-source-comment-bar)": {
      border: "none",
      background: "none",
      boxShadow: "none",
    },
  });

  return [field, theme];
}
