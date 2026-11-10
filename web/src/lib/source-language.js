import { Prec } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { json } from "@codemirror/lang-json";
import { yamlFrontmatter } from "@codemirror/lang-yaml";
import { languages } from "@codemirror/language-data";
import { LanguageDescription } from "@codemirror/language";
import { typst } from "./typst-language/index.js";
import { sourceFormatOf } from "./formats.js";
import { pandocExtensions } from "./pandoc-markdown.js";
import { quartoOptions } from "./quarto-options.js";
import { latex } from "./latex-language/index.js";
import { org } from "./org-language/index.js";
import { pipeTableAt, pipeTableKeymap } from "./pipe-table-keymap.js";

/** Resolve an exact language name for a Markdown or Quarto code fence. */
export function codeLanguage(info) {
  if (typeof info !== "string") return null;
  const raw = info.trim();
  if (!raw) return null;

  // Markdown fences start with a bare info string (`python`). Quarto's
  // executable chunks put the engine and options inside braces (`{r}` or
  // `{python label="figure"}`). Keep only the first token and match it
  // exactly so an unknown name never accidentally selects a similar mode.
  let name = raw;
  if (name.startsWith("{")) {
    const close = name.indexOf("}");
    // CodeMirror currently reduces a fence info string to its first token
    // before calling this resolver. Thus `{python label=...}` arrives as
    // `{python`; accept that useful prefix, as well as a first token ending
    // in the comma from `{r, echo=FALSE}`.
    name = name.slice(1, close < 0 ? undefined : close).trim();
  }
  name = name.split(/[\s,]+/, 1)[0]?.toLowerCase() || "";
  if (!name) return null;
  return LanguageDescription.matchLanguageName(languages, name, false) || null;
}

// GitHub-flavoured Markdown (tables, strikethrough, task lists and autolinks) with
// Pandoc and Quarto syntax (math, fenced divs, citations, attributes).
// The base is required because markdown() defaults to plain CommonMark.
const markdownSupport = () => yamlFrontmatter({
  content: markdown({ base: markdownLanguage, codeLanguages: codeLanguage, extensions: pandocExtensions }),
});

// Tab, Shift-Tab and Enter move through pipe tables; outside a table they fall through.
const markdownTables = () => Prec.high(keymap.of(pipeTableKeymap("markdown", pipeTableAt)));

function byFormat(format) {
  switch ((format || "").toLowerCase()) {
    case "latex":
    case "tex": return latex();
    case "typst": return typst();
    case "org": return org();
    case "html": return html();
    case "quarto": return [markdownSupport(), markdownTables(), quartoOptions];
    case "markdown":
    case "md": return [markdownSupport(), markdownTables()];
    default: return [];
  }
}

/**
 * The format sourceLanguage() picks for a path: "latex", "typst", "html",
 * "quarto", "markdown", "json", or "" for plain text.
 * @param {string} path
 * @param {string} [fallback] The declared format, for empty paths and .txt.
 */
export function sourceFormat(path, fallback = "") {
  const lower = typeof path === "string" ? path.toLowerCase() : "";
  const extension = lower.slice(lower.lastIndexOf("/") + 1);
  const format = sourceFormatOf(extension);
  if (format) return format;
  if (/\.json$/.test(extension)) return "json";

  // Named files of an unsupported type are plain text. Empty paths and .txt
  // retain the document's declared format, as they did in the editor before.
  if (extension && !extension.endsWith(".txt")) return "";
  const declared = (fallback || "").toLowerCase();
  if (declared === "tex") return "latex";
  if (declared === "md") return "markdown";
  return declared;
}

/** Select a CodeMirror language from a file path, falling back for unnamed text. */
export function sourceLanguage(path, fallback = "") {
  const format = sourceFormat(path, fallback);
  return format === "json" ? json() : byFormat(format);
}
