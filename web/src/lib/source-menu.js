import { EditorView } from "@codemirror/view";

// The right-click menu of the source editor: what it offers, where the caret
// goes when a click lands outside the selection, and whether the browser lets
// the editor read the clipboard for Paste. The names are lib/commands.js's, so
// the menu prints the same keys the registry binds.

/// The menu's items, in the order it draws them, as groups divided by rules.
export const SOURCE_MENU = [["comment", "link"], ["cut", "copy", "paste"]];

/// Where a right-click at `pos` leaves the caret, or null to leave it where it
/// is. A click inside the selection keeps the selection, so the menu acts on
/// what the person pointed at; a click anywhere else moves the caret there
/// first, as Google Docs does. A click that found no text (`pos` null) moves nothing.
export function rightClickSelection(range, pos) {
  if (pos === null || pos === undefined) return null;
  if (pos >= range.from && pos <= range.to) return null;
  return pos;
}

/// Whether the browser lets the page read the clipboard. False where the API
/// is missing (an insecure page, an old browser); a permission the person has
/// refused is also false. A browser that does not answer the permission query
/// is taken as readable: a refused read greys Paste when it happens.
export async function clipboardReadable(nav = globalThis.navigator) {
  if (typeof nav?.clipboard?.readText !== "function") return false;
  try {
    const status = await nav.permissions?.query({ name: /** @type {PermissionName} */ ("clipboard-read") });
    return status?.state !== "denied";
  } catch {
    return true;
  }
}

/// The right-button press moves the caret when it lands outside the selection.
/// CodeMirror ignores a press that is not the primary button, so the selection
/// would otherwise stay wherever the last left click put it. The press is not
/// cancelled: the browser still focuses the editor and opens the menu.
export function rightClickCaret() {
  return EditorView.domEventHandlers({
    mousedown(event, view) {
      if (event.button !== 2) return false;
      const range = view.state.selection.main;
      const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
      const anchor = rightClickSelection(range, pos);
      if (anchor !== null) view.dispatch({ selection: { anchor }, userEvent: "select" });
      return false;
    },
  });
}
