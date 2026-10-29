import { WidgetType } from "@codemirror/view";
import { typst2tex } from "tex2typst";
import "./vendor/styles.css";
import "./visual-markup.css";
import { preambleFold } from "./preamble.js";
import {
  createImageResolver,
  createTheme,
  DARK_THEME_COLORS,
  DEFAULT_CONFIG,
  imageResolver,
  LIGHT_THEME_COLORS,
  registerWidget,
  textOf,
  visualDefaults,
  visualExtension,
} from "./vendor/index.ts";
import { loadKatex } from "../katex.js";

/// The formats the visual layer draws.
export const VISUAL_FORMATS = new Set(["latex", "typst"]);

// LaTeX's \includegraphics often leaves the extension off, so these are tried in turn.
const IMAGE_EXTENSIONS = [".png", ".jpg", ".jpeg", ".svg", ".pdf"];

/// The visual layer for one file, or nothing for a format it does not draw.
/// `path` returns the open file's project path (no leading slash), for resolving relative image paths.
/// `imageUrl(path)` resolves a project path (no leading slash) to a URL an <img> can load, or null.
/// `dark` picks the dark colours.
export function visualMarkup(format, { path = () => "", imageUrl = null, dark = false } = {}) {
  if (!VISUAL_FORMATS.has(format)) return [];

  const extensions = [
    visualExtension(),
    visualDefaults.of({ enabled: true, language: format }),
    createTheme({
      ...DEFAULT_CONFIG,
      theme: dark ? "dark" : "light",
      styles: {
        ...DEFAULT_CONFIG.styles,
        colors: dark ? DARK_THEME_COLORS : LIGHT_THEME_COLORS,
      },
    }),
  ];

  if (imageUrl) {
    // Upstream resolves against leading-slash paths; LibrePaper's have none.
    extensions.push(
      imageResolver.of(
        createImageResolver(
          () => "/" + path(),
          (resolved) => resolveImage(imageUrl, resolved.replace(/^\/+/, "")),
        ),
      ),
    );
  }

  if (format === "latex") extensions.push(preambleFold());

  return extensions;
}

async function resolveImage(imageUrl, file) {
  const url = await imageUrl(file);
  if (url || /\.[^/]+$/.test(file)) return url;
  for (const extension of IMAGE_EXTENSIONS) {
    const found = await imageUrl(file + extension);
    if (found) return found;
  }
  return null;
}

// KaTeX lacks some math environments and has no \label. The environment is
// rebuilt in a form KaTeX accepts, then the labels are removed.
function latexMath(body, environment) {
  let tex;
  if (!environment || environment === "equation" || environment === "equation*") {
    tex = body;
  } else if (environment === "multline" || environment === "multline*") {
    tex = `\\begin{gather*}${body}\\end{gather*}`;
  } else if (environment === "eqnarray" || environment === "eqnarray*") {
    tex = `\\begin{array}{rcl}${body}\\end{array}`;
  } else {
    // align, align*, gather and gather* are KaTeX's own environments.
    tex = `\\begin{${environment}}${body}\\end{${environment}}`;
  }
  return tex.replace(/\\label\s*\{[^{}]*\}/g, "");
}

// Typesets a math token with KaTeX. The raw source shows until KaTeX loads,
// and stays if it fails.
class KatexMath extends WidgetType {
  constructor(context) {
    super();
    const { token, source, language } = context;
    this.text = textOf(source, token);
    this.tex = token.body ? textOf(source, token.body) : this.text;
    this.environment = token.name;
    this.display = Boolean(token.display);
    this.language = language.id;
  }

  eq(other) {
    return (
      other instanceof KatexMath &&
      other.text === this.text &&
      other.display === this.display &&
      other.language === this.language
    );
  }

  toDOM() {
    const element = document.createElement(this.display ? "div" : "span");
    element.className = `cm-lv-widget cm-lv-math ${this.display ? "cm-lv-math-display" : "cm-lv-math-inline"}`;
    element.textContent = this.text;

    loadKatex()
      .then((katex) => {
        if (!element.isConnected) return;
        katex.render(this.toTex(), element, {
          displayMode: this.display,
          throwOnError: false,
        });
      })
      .catch(() => {});

    return element;
  }

  // LaTeX is rebuilt for KaTeX; Typst source is converted to TeX, and on
  // failure the raw text stays.
  toTex() {
    if (this.language !== "typst") return latexMath(this.tex, this.environment);
    try {
      return typst2tex(this.tex, { blockMathMode: this.display });
    } catch {
      return this.text;
    }
  }

  // A click places the caret, which makes the vendor reveal the source.
  ignoreEvent() {
    return false;
  }
}

// Runs after the vendor import, which registers upstream's math widget; this
// overwrites it.
registerWidget("math", (context) => new KatexMath(context));
