// A pinch or ctrl+wheel over the source sizes its text. The size is a habit of
// the person looking at the source, so it is remembered in this browser only.
import { EditorView } from "@codemirror/view";
import { watchPinch } from "./pinch.js";
import { read, write, EDITOR_SIZE } from "./storage.js";

export const DEFAULT_SIZE = 14;
export const MIN_SIZE = 10;
export const MAX_SIZE = 28;

/// A size in pixels kept between MIN_SIZE and MAX_SIZE. Anything that is not a finite number is DEFAULT_SIZE.
export function clampSize(size) {
  if (typeof size !== "number" || !Number.isFinite(size)) return DEFAULT_SIZE;
  return Math.min(MAX_SIZE, Math.max(MIN_SIZE, size));
}

/// The size a gesture that started at `base` and has scaled by `factor` asks for, clamped (not rounded).
export function sizeAfter(base, factor) {
  return clampSize(base * factor);
}

/// The size this browser remembered, or DEFAULT_SIZE. Rounded to a whole pixel and clamped.
export function storedSize() {
  const size = read(EDITOR_SIZE, DEFAULT_SIZE);
  return clampSize(typeof size === "number" ? Math.round(size) : DEFAULT_SIZE);
}

/// Pinch and ctrl+wheel over `host` change the editor's text size. `getView()` returns the current EditorView or null. Returns stop().
export function attachEditorZoom(host, getView) {
  let size = storedSize();
  let gesture = null; // { base, anchor, offset } while a gesture is active
  host.style.setProperty("--editor-font-size", `${size}px`);

  const apply = (factor, x, y) => {
    const view = getView();
    if (!gesture) {
      gesture = { base: size, anchor: null, offset: 0 };
      if (view) {
        gesture.anchor = view.posAtCoords({ x, y });
        gesture.offset = y - view.scrollDOM.getBoundingClientRect().top;
      }
    }
    size = sizeAfter(gesture.base, factor);
    host.style.setProperty("--editor-font-size", `${size}px`);
    if (view) {
      view.requestMeasure();
      if (gesture.anchor !== null) {
        view.dispatch({
          effects: EditorView.scrollIntoView(gesture.anchor, { y: "start", yMargin: gesture.offset }),
        });
      }
    }
  };

  const stop = watchPinch(host, {
    onchange: (factor, x, y) => apply(factor, x, y),
    onend: (factor, x, y) => {
      apply(factor, x, y);
      size = Math.round(size);
      host.style.setProperty("--editor-font-size", `${size}px`);
      write(EDITOR_SIZE, size);
      gesture = null;
    },
  });
  return stop;
}
