// The visual mode of Markdown, Quarto, Org and Typst files (LaTeX keeps lib/visual-markup).
// Each top-level block that the selection is not in is drawn as the HTML the
// renderer produces for it, styled with the preview page's own CSS. The block
// holding the selection shows its source. The text stays the document; only
// the drawing differs.
//
// Overlays (comments, tracked changes, authorship, lint underlines, remote
// carets) cannot reach inside a widget, so overlays.js paints copies into the
// rendered DOM.
import { Prec, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, keymap, ViewPlugin, WidgetType } from "@codemirror/view";
import { caretParked, isExternal, resolveImagePath, revealingSelection, stepIntoBlocksPlugin } from "../visual-markup/markup.ts";
import "../visual-markup/visual-markup.css";
import { loadKatex } from "../katex.js";
import { applyNumbering, displaySources, typeset, untypeset } from "../math.js";
import { numberDisplays } from "../math-numbering.js";
import { foldLayer, foldsOf } from "../visual/folds.js";
import { currentPaperRenderer, loadPaperRenderer } from "./renderer.js";
import { paperFormat } from "./formats.js";
import { paperStylesheet } from "./style.js";
import { paperOverlays } from "./overlays.js";

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

// The TeX of a block's display formulas, by the HTML the block was drawn as, so a selection change does not parse every block again.
const sourcesByHtml = new Map();

function displaySourcesOf(html) {
  let sources = sourcesByHtml.get(html);
  if (!sources) {
    const template = document.createElement("template");
    template.innerHTML = html;
    sources = displaySources(template.content);
    if (sourcesByHtml.size >= 1000) sourcesByHtml.clear();
    sourcesByHtml.set(html, sources);
  }
  return sources;
}

// Numbers the display formulas of every drawn block together, so a counter runs down the document, and returns each block's share.
function numberBlocks(htmls) {
  const entries = [...htmls].map(([block, html]) => ({ block, sources: displaySourcesOf(html) }));
  const { tex, keys, labels } = numberDisplays(entries.flatMap((entry) => entry.sources));
  const shares = new Map();
  let offset = 0;
  for (const { block, sources } of entries) {
    const end = offset + sources.length;
    shares.set(block, { tex: tex.slice(offset, end), keys: keys.slice(offset, end), labels });
    offset = end;
  }
  return shares;
}

class PaperWidget extends WidgetType {
  constructor(html, from, path, imageUrl, math = null) {
    super();
    this.html = html;
    this.from = from;
    this.path = path;
    this.imageUrl = imageUrl;
    this.math = math;
    // Compared as text, so that a renumbering redraws the block.
    this.mathKey = math ? JSON.stringify([math.tex, math.keys, [...math.labels]]) : "";
  }

  eq(other) {
    return other.html === this.html && other.mathKey === this.mathKey;
  }

  toDOM() {
    const div = document.createElement("div");
    div.className = "cm-lp-paper";
    div.innerHTML = this.html;
    // The numbers go in before KaTeX is here, so a link to an equation reads its number at once.
    const numbering = this.math ? applyNumbering(div, document, this.math) : null;

    if (untypeset(div).length) {
      loadKatex()
        .then((katex) => typeset(katex, untypeset(div), numbering))
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

function build(state, path, imageUrl, adapter, project) {
  const renderer = currentPaperRenderer(adapter.module);
  if (!renderer) return { decorations: Decoration.none, rendered: [], gaps: new Set() };
  // A renderer that compiles in the project (Typst) is told which one before it draws.
  if (project && renderer.setProject) renderer.setProject(project());
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
  if (!blocks) return { decorations: Decoration.none, rendered: [], gaps: new Set() };
  // Every block the renderer draws, the selected ones too, so that numbers do not move when the caret enters an equation.
  const htmls = new Map();
  for (const block of blocks) {
    if (!block.render) continue;
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
    htmls.set(block, html);
  }
  // Display math is numbered across the document as the preview numbers it. Typst's math is not KaTeX's, so it is left alone.
  const numbering = adapter.module !== "typst" ? numberBlocks(htmls) : null;
  // A parked caret (the layer just turned on) reveals nothing, as in the LaTeX layer.
  const ranges = revealingSelection(state);
  // A block that overlaps a fold stays source: the fold layer draws that line, or shows the source when revealed.
  const folds = foldsOf(state);
  const marks = [];
  const rendered = [];
  for (const block of blocks) {
    if (!htmls.has(block) || touches(ranges, block)) continue;
    if (folds.some((fold) => block.from <= fold.to && block.to >= fold.from)) continue;
    const html = htmls.get(block);
    if (!html.trim()) continue;
    rendered.push(block);
    marks.push(
      Decoration.replace({
        widget: new PaperWidget(html, block.from, path, imageUrl, numbering?.get(block) ?? null),
        block: true,
      }).range(block.from, block.to),
    );
  }
  // Blank source lines between two drawn blocks would add a full line to the preview's spacing, so they are collapsed.
  // The collapsed lines are kept too: a keyboard move passes over them as if they were not there.
  const drawn = new Set(rendered);
  const gaps = new Set();
  for (let i = 0; i + 1 < blocks.length; i++) {
    if (!drawn.has(blocks[i]) || !drawn.has(blocks[i + 1])) continue;
    const last = state.doc.lineAt(blocks[i + 1].from).number;
    for (let number = state.doc.lineAt(blocks[i].to).number + 1; number < last; number++) {
      const line = state.doc.line(number);
      if (/^\s*$/.test(line.text) && !touches(ranges, line)) {
        marks.push(Decoration.line({ class: "cm-lp-paper-gap" }).range(line.from));
        gaps.add(line.from);
      }
    }
  }
  return { decorations: Decoration.set(marks, true), rendered, gaps };
}

function paperField(path, imageUrl, adapter, project) {
  return StateField.define({
    create: (state) => build(state, path, imageUrl, adapter, project),
    update(value, transaction) {
      if (
        transaction.docChanged ||
        transaction.selection ||
        transaction.effects.some((effect) => effect.is(refreshPaper))
      ) {
        return build(transaction.state, path, imageUrl, adapter, project);
      }
      return value;
    },
    provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
  });
}

// Loads the renderer and the format's syntax tree once per view, injects the page CSS once per document, and redraws.
function loaderFor(adapter, project) {
  return ViewPlugin.define((view) => {
    // The images and packages the project needs arrive after the first draw; each arrival redraws.
    const prepare = () => {
      const renderer = currentPaperRenderer(adapter.module);
      if (!project || !renderer || !adapter.prepare) return;
      try {
        adapter.prepare(view.state.doc.toString(), renderer, project(), () => {
          if (!plugin.destroyed) view.dispatch({ effects: refreshPaper.of(null) });
        });
      } catch {
        // The view keeps showing source for what cannot be prepared.
      }
    };
    const plugin = {
      destroyed: false,
      destroy() { plugin.destroyed = true; },
      update(update) {
        if (update.docChanged) prepare();
      },
    };
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
        prepare();
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
/// `project()` returns `{ main, texts, assets, assetUrl }` for a format that compiles in the project (Typst):
/// `main` the open file's path, `texts` every project text by path, `assets` each asset path to a version,
/// `assetUrl(path)` a fetchable URL for an asset or null.
/// `dark` puts `lp-dark` on the editor, which the house stylesheet answers.
/// `openInclude(name)` opens the file a fold's include names; without it the include fold has no button.
/// The fold layer comes before the paper field, so the field sees the folds in force when it builds.
export function paperVisual({ format = "markdown", path = () => "", imageUrl = null, dark = false, project = null, openInclude = null } = {}) {
  const adapter = paperFormat(format);
  if (!adapter) return [];
  const field = paperField(path, imageUrl, adapter, project);
  return [
    caretParked,
    ...foldLayer(adapter.folds, { openInclude }),
    field,
    // Vim's j and k bypass the keymap below; this puts the caret on the first line of a skipped block.
    // A line counts as unseen when it is a collapsed gap before or after the move, or when it was drawn as part of a block before the move.
    stepIntoBlocksPlugin({
      hidden: (state, line, before) =>
        state.field(field).gaps.has(line.from) ||
        before.field(field).gaps.has(line.from) ||
        before.field(field).rendered.some((block) => block.from <= line.from && line.from <= block.to),
    }),
    loaderFor(adapter, project),
    paperOverlays(field),
    EditorView.editorAttributes.of({
      class: dark ? "cm-lp-visual cm-lp-paper-editor lp-dark" : "cm-lp-visual cm-lp-paper-editor",
    }),
    EditorView.domEventHandlers({
      mousedown(event, view) {
        const target = event.target;
        if (!(target instanceof Element)) return false;
        const div = target.closest(".cm-lp-paper");
        if (!div) return false;
        // A painted comment or tracked change answers a click as it does in the source:
        // the same click reaches the editor's handlers. The caret stays out of the block,
        // so the block stays drawn.
        const painted = target.closest("[data-lp-overlay][data-comment-id], [data-lp-overlay][data-change]");
        if (painted && div.contains(painted)) {
          event.preventDefault();
          painted.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, clientX: event.clientX, clientY: event.clientY, detail: 1 }));
          return true;
        }
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
