// A pinch or ctrl+wheel over the source sizes its text. The size itself lives in
// editor-type.js, which the settings row changes too, so a change made there, or
// in another tab, moves the text here as well. The size is remembered in this
// browser only.
import { EditorView } from "@codemirror/view";
import { watchPinch } from "./pinch.js";
import { clampSize, size, setSize, onSize } from "./editor-type.js";

/// The size a gesture that started at `base` and has scaled by `factor` asks for, clamped (not rounded).
export function sizeAfter(base, factor) {
  return clampSize(base * factor);
}

/// Pinch and ctrl+wheel over `host` change the editor's text size. `getView()` returns the current EditorView or null. Returns stop().
export function attachEditorZoom(host, getView) {
  let current = size();
  let gesture = null; // { base, anchor, offset } while a gesture is active
  host.style.setProperty("--editor-font-size", `${current}px`);

  // A change from the settings row or another tab. It waits while a gesture is
  // in progress, so the gesture keeps its own size until it ends.
  const stopSize = onSize((next) => {
    if (gesture) return;
    current = next;
    host.style.setProperty("--editor-font-size", `${current}px`);
    getView()?.requestMeasure();
  });

  const apply = (factor, x, y) => {
    const view = getView();
    if (!gesture) {
      gesture = { base: current, anchor: null, offset: 0 };
      if (view) {
        gesture.anchor = view.posAtCoords({ x, y });
        gesture.offset = y - view.scrollDOM.getBoundingClientRect().top;
      }
    }
    current = sizeAfter(gesture.base, factor);
    host.style.setProperty("--editor-font-size", `${current}px`);
    if (view) {
      view.requestMeasure();
      if (gesture.anchor !== null) {
        view.dispatch({
          effects: EditorView.scrollIntoView(gesture.anchor, { y: "start", yMargin: gesture.offset }),
        });
      }
    }
  };

  const stopPinch = watchPinch(host, {
    onchange: (factor, x, y) => apply(factor, x, y),
    onend: (factor, x, y) => {
      apply(factor, x, y);
      current = Math.round(current);
      host.style.setProperty("--editor-font-size", `${current}px`);
      gesture = null;
      setSize(current);
    },
  });
  return () => {
    stopPinch();
    stopSize();
  };
}
