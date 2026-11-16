// The on-screen keyboard takes up part of the phone's screen without the page
// being resized to fit it. The visual viewport is the part still showing, so
// the difference between the window and that viewport is what the keyboard
// covers. Smaller gaps are the browser's own toolbars and are not counted.

export const KEYBOARD_THRESHOLD = 120;

// Input types that take a click or a key but never text, so no keyboard opens for them.
const NOT_TEXT_INPUTS = ["button", "checkbox", "radio", "submit", "range", "color", "file"];

/// Pixels of the window the on-screen keyboard covers. A pinch zoom also shrinks the visual viewport, and that is not the keyboard, so a zoomed page reports 0.
export function keyboardInset(innerHeight, viewport) {
  if (!viewport || viewport.scale > 1.01) return 0;
  return Math.round(Math.max(0, innerHeight - viewport.height - viewport.offsetTop));
}

/// Whether focusing `element` brings up the keyboard.
export function isEditable(element) {
  if (!element) return false;
  if (element.isContentEditable === true) return true;
  const tag = element.tagName;
  if (tag === "TEXTAREA") return true;
  if (tag === "INPUT") return !NOT_TEXT_INPUTS.includes(element.type);
  return false;
}

/// Calls `onchange(open, inset)` when the keyboard opens, closes or changes height, and keeps --librepaper-keyboard on the root element in step. Returns stop().
export function watchKeyboard(onchange) {
  const viewport = window.visualViewport;
  if (!viewport) return () => {};

  const root = document.documentElement;
  let open = false;
  let inset = 0;
  let pending = null;

  function measure() {
    const measured = keyboardInset(window.innerHeight, window.visualViewport);
    const nowOpen = measured >= KEYBOARD_THRESHOLD && isEditable(document.activeElement);
    const reported = nowOpen ? measured : 0;
    root.style.setProperty("--librepaper-keyboard", `${reported}px`);
    // iOS scrolls the page itself to show a focused field, and the page never
    // scrolls on its own, so any scroll while the keyboard is up is undone.
    if (nowOpen && window.scrollY > 0) window.scrollTo(0, 0);
    if (nowOpen !== open || reported !== inset) {
      open = nowOpen;
      inset = reported;
      onchange(open, inset);
    }
  }

  // Focus moves before the browser has settled activeElement, so the check
  // waits a tick.
  function onFocusChange() {
    clearTimeout(pending);
    pending = setTimeout(() => {
      pending = null;
      measure();
    }, 0);
  }

  viewport.addEventListener("resize", measure);
  viewport.addEventListener("scroll", measure);
  document.addEventListener("focusin", onFocusChange);
  document.addEventListener("focusout", onFocusChange);
  measure();

  return function stop() {
    viewport.removeEventListener("resize", measure);
    viewport.removeEventListener("scroll", measure);
    document.removeEventListener("focusin", onFocusChange);
    document.removeEventListener("focusout", onFocusChange);
    clearTimeout(pending);
    pending = null;
    root.style.removeProperty("--librepaper-keyboard");
  };
}
