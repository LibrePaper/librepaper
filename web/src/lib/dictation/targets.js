// Where dictated text lands.
//
// A target hides the difference between a plain form control and the
// CodeMirror editor behind one shape -- { insert, before, alive, focus } --
// so service.js never branches on what it is talking to. Everything here is
// pure DOM/editor-object manipulation with no browser globals captured at
// module load, so `web/tests/integration/dictation-service.mjs` can build targets over
// small fake elements/editors under Node.

/// A <textarea> or a text <input>. Insertion replaces the current selection
/// (an empty selection is just the caret) and leaves the caret after the
/// inserted text, then fires a bubbling `input` event so Svelte's
/// `bind:value` and any `oninput` composer handler observe the change --
/// setting `.value` directly does neither.
export function textareaTarget(element) {
  return {
    kind: "textarea",
    insert(text) {
      const start = element.selectionStart ?? element.value.length;
      const end = element.selectionEnd ?? element.value.length;
      const value = element.value;
      element.value = value.slice(0, start) + text + value.slice(end);
      const caret = start + text.length;
      if (typeof element.setSelectionRange === "function") {
        element.setSelectionRange(caret, caret);
      } else {
        element.selectionStart = caret;
        element.selectionEnd = caret;
      }
      element.dispatchEvent(new Event("input", { bubbles: true }));
    },
    before(limit = 200) {
      const start = element.selectionStart ?? element.value.length;
      return element.value.slice(Math.max(0, start - limit), start);
    },
    alive() {
      return element.isConnected;
    },
    focus() {
      element.focus();
    },
  };
}

/// Editor.svelte's own surface: `insertAtCaret`, `textBeforeCaret`,
/// `vimMode()`, `focus()`. `insert` throws a `VimNormalMode` error
/// rather than inserting keystrokes into normal mode; the service turns
/// that into the toast telling the user to enter insert mode.
export function editorTarget(editor) {
  return {
    kind: "editor",
    insert(text) {
      if (editor.vimMode() === "normal") {
        const error = new Error("The editor is in Vim normal mode");
        error.name = "VimNormalMode";
        throw error;
      }
      editor.insertAtCaret(text);
    },
    before(limit = 200) {
      return editor.textBeforeCaret(limit);
    },
    alive() {
      return editor.alive ? editor.alive() : true;
    },
    focus() {
      editor.focus();
    },
  };
}

/// What the toolbar button and the Ctrl+Shift+D shortcut bind to: whichever
/// target the currently focused element implies, or `null` when it implies
/// none (the shortcut then toasts "Click into a text field
/// first"). `editorFor(element)` answers whether `element` is (or is inside)
/// a known editor's DOM, returning that editor or null; a plain textarea/text
/// input wins on its own without consulting it.
export function targetForActiveElement(doc, editorFor) {
  const active = doc.activeElement;
  if (!active) return null;
  const tag = active.tagName ? active.tagName.toLowerCase() : "";
  if (tag === "textarea" || (tag === "input" && isTextualInput(active))) {
    return textareaTarget(active);
  }
  const editor = editorFor(active);
  if (editor) return editorTarget(editor);
  return null;
}

const TEXTUAL_INPUT_TYPES = new Set(["text", "search", "url", "tel", "", undefined]);

function isTextualInput(element) {
  const type = (element.type || "text").toLowerCase();
  return TEXTUAL_INPUT_TYPES.has(type);
}
