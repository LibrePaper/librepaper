// The paper visual layer for a Markdown or Org file: each top-level block that the
// selection is not in is drawn as the HTML the preview renderer produces for
// it, styled with the preview page's own CSS. The block holding the selection
// shows its Markdown or Org source. The text stays the document; only the drawing differs.
//
// Prototype limits: overlays (comments, tracked changes, spellcheck, remote
// cursors) do not draw inside rendered blocks yet.
import { Prec, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, keymap, ViewPlugin, WidgetType } from "@codemirror/view";
import { isExternal, resolveImagePath } from "../visual-markup/markup.ts";
import "../visual-markup/visual-markup.css";
import { loadKatex } from "../katex.js";
import { typeset, untypeset } from "../math.js";
import { currentPaperRenderer, loadPaperRenderer } from "./renderer.js";
import { paperFormat } from "./formats.js";
import { paperStylesheet } from "./style.js";

const refreshPaper = StateEffect.define();

const IMAGE_EXTENSIONS = [".png", ".jpg", ".jpeg", ".svg", ".pdf"];

async function resolveImage(imageUrl, file) {
  const url = await imageUrl(file);
  if (url || /\.[^/]+$/.test(file)) return url;
  for (const extension of IMAGE_EXTENSIONS) {
    const found = await imageUrl(file + extension);
    if (found) return found;
  }
  return null;
}

class PaperWidget extends WidgetType {
  constructor(html, from, path, imageUrl) {
    super();
    this.html = html;
    this.from = from;
    this.path = path;
    this.imageUrl = imageUrl;
  }

  eq(other) {
    return other.html === this.html;
  }

  toDOM() {
    const div = document.createElement("div");
    div.className = "cm-lp-paper";
    div.innerHTML = this.html;

    if (untypeset(div).length) {
      loadKatex()
        .then((katex) => typeset(katex, untypeset(div), null))
        .catch(() => {});
    }

    if (this.imageUrl) {
      for (const img of div.querySelectorAll("img")) {
        const src = img.getAttribute("src");
        if (!src || isExternal(src)) continue;
        const resolved = resolveImagePath("/" + this.path(), src).replace(/^\/+/, "");
        resolveImage(this.imageUrl, resolved)
          .then((url) => {
            if (url) img.src = url;
          })
          .catch(() => {});
      }
    }

    div.addEventListener("click", (event) => {
      if (event.target instanceof Element && event.target.closest("a")) event.preventDefault();
    });
    return div;
  }

  ignoreEvent() {
    return false;
  }

  get estimatedHeight() {
    return -1;
  }
}

function touches(ranges, block) {
  return ranges.some((range) => range.from <= block.to && range.to >= block.from);
}

function build(state, path, imageUrl, adapter) {
  const renderer = currentPaperRenderer(adapter.module);
  if (!renderer) return { decorations: Decoration.none, rendered: [] };
  const text = state.doc.toString();
  // The whole document rendered at once when the format can; null falls back to drawing each block alone.
  let whole = null;
  if (adapter.renderedBlocks) {
    try {
      whole = adapter.renderedBlocks(text, renderer, path());
    } catch {
      whole = null;
    }
  }
  const blocks = whole ?? adapter.splitBlocks(text);
  if (!blocks) return { decorations: Decoration.none, rendered: [] };
  const ranges = state.selection.ranges;
  const marks = [];
  const rendered = [];
  for (const block of blocks) {
    if (!block.render || touches(ranges, block)) continue;
    let html = "";
    if (whole) {
      html = block.html ?? "";
    } else {
      try {
        html = renderer.render(adapter.blockSource(text, block, blocks));
      } catch {
        continue;
      }
    }
    if (!html.trim()) continue;
    rendered.push(block);
    marks.push(
      Decoration.replace({ widget: new PaperWidget(html, block.from, path, imageUrl), block: true }).range(
        block.from,
        block.to,
      ),
    );
  }
  return { decorations: Decoration.set(marks), rendered };
}

function paperField(path, imageUrl, adapter) {
  return StateField.define({
    create: (state) => build(state, path, imageUrl, adapter),
    update(value, transaction) {
      if (
        transaction.docChanged ||
        transaction.selection ||
        transaction.effects.some((effect) => effect.is(refreshPaper))
      ) {
        return build(transaction.state, path, imageUrl, adapter);
      }
      return value;
    },
    provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
  });
}

// Loads the renderer and the format's syntax tree once per view, injects the page CSS once per document, and redraws.
function loaderFor(adapter) {
  return ViewPlugin.define((view) => {
    const plugin = { destroyed: false, destroy() { plugin.destroyed = true; } };
    // The page stylesheet is always the Markdown renderer's: it is the house
    // style every format shares, whichever renderer draws this file.
    const house = adapter.module === "markdown" ? null : loadPaperRenderer("markdown").catch(() => null);
    Promise.all([loadPaperRenderer(adapter.module), adapter.ready(), house])
      .then(([renderer, , markdown]) => {
        if (plugin.destroyed) return;
        const pageCss = (adapter.module === "markdown" ? renderer : markdown)?.pageCss;
        if (pageCss && !document.head.querySelector("style[data-librepaper-paper]")) {
          const style = document.createElement("style");
          style.setAttribute("data-librepaper-paper", "");
          style.textContent = paperStylesheet(pageCss);
          document.head.appendChild(style);
        }
        // The Markdown page stylesheet does not cover Typst's MathML, so Typst brings its own rules.
        if (adapter.module === "typst" && renderer.mathCss && !document.head.querySelector("style[data-librepaper-paper-typst]")) {
          const style = document.createElement("style");
          style.setAttribute("data-librepaper-paper-typst", "");
          style.textContent = renderer.mathCss;
          document.head.appendChild(style);
        }
        view.dispatch({ effects: refreshPaper.of(null) });
      })
      .catch(() => {});
    return plugin;
  });
}

function stepInto(field, direction) {
  return (view) => {
    const selection = view.state.selection;
    if (selection.ranges.length !== 1 || !selection.main.empty) return false;
    const { doc } = view.state;
    const line = doc.lineAt(selection.main.head);
    // Inside a wrapped line, the default motion moves between its rows.
    const moved = view.moveVertically(selection.main, direction > 0);
    if (moved.head >= line.from && moved.head <= line.to && moved.head !== selection.main.head) return false;
    const rendered = view.state.field(field).rendered;
    if (direction > 0) {
      if (line.number >= doc.lines) return false;
      const next = doc.line(line.number + 1);
      const block = rendered.find((candidate) => candidate.from === next.from);
      if (!block) return false;
      view.dispatch({ selection: { anchor: block.from }, scrollIntoView: true });
      return true;
    }
    if (line.number <= 1) return false;
    const previous = doc.line(line.number - 1);
    const block = rendered.find((candidate) => candidate.to === previous.to);
    if (!block) return false;
    view.dispatch({ selection: { anchor: doc.lineAt(block.to).from }, scrollIntoView: true });
    return true;
  };
}

/// The paper visual layer for a Markdown or Org file. `format` names the file's format ("markdown" or "org").
/// `path` returns the open file's project path (no leading slash).
/// `imageUrl(path)` resolves a project path to a URL for an <img>, or null; it may be async.
/// `dark` picks the dark rules.
export function paperVisual({ format = "markdown", path = () => "", imageUrl = null, dark = false } = {}) {
  const adapter = paperFormat(format);
  if (!adapter) return [];
  const field = paperField(path, imageUrl, adapter);
  return [
    field,
    loaderFor(adapter),
    EditorView.editorAttributes.of({
      class: dark ? "cm-lp-visual cm-lp-paper-editor cm-lp-paper-dark" : "cm-lp-visual cm-lp-paper-editor",
    }),
    EditorView.domEventHandlers({
      mousedown(event, view) {
        const target = event.target;
        if (!(target instanceof Element)) return false;
        const div = target.closest(".cm-lp-paper");
        if (!div) return false;
        let pos;
        try {
          pos = view.posAtDOM(div);
        } catch {
          return false;
        }
        event.preventDefault();
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
        view.focus();
        return true;
      },
    }),
    Prec.high(
      keymap.of([
        { key: "ArrowDown", run: stepInto(field, 1) },
        { key: "ArrowUp", run: stepInto(field, -1) },
      ]),
    ),
  ];
}
