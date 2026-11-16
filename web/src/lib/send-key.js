// The one send key for every comment and chat box.
//
// Enter sends. Ctrl+Enter and Cmd+Enter send too, so the habit from other
// chat tools works. Shift+Enter is the newline. While an IME is composing the
// Enter belongs to the input method, so it never sends.
//
// Callers still decide what "send" means: an empty or whitespace-only box
// does nothing, and each box checks that itself.
export function isSendKey(event) {
  if (event.key !== "Enter" || event.shiftKey) return false;
  if (event.isComposing || event.keyCode === 229) return false;
  return true;
}
