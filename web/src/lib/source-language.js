import { StreamLanguage } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { json } from "@codemirror/lang-json";
import { yamlFrontmatter } from "@codemirror/lang-yaml";
import { languages } from "@codemirror/language-data";
import { LanguageDescription } from "@codemirror/language";
import { typstLanguage } from "./typst-mode.js";
import { sourceFormatOf } from "./formats.js";
import { pandocExtensions } from "./pandoc-markdown.js";
import { latex } from "./latex-language/index.js";

const typst = StreamLanguage.define(typstLanguage);

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

function byFormat(format) {
  switch ((format || "").toLowerCase()) {
    case "latex":
    case "tex": return latex();
    case "typst": return typst;
    case "html": return html();
    case "quarto":
    case "markdown":
    case "md": return markdownSupport();
    default: return [];
  }
}

/** Select a CodeMirror language from a file path, falling back for unnamed text. */
export function sourceLanguage(path, fallback = "") {
  const lower = typeof path === "string" ? path.toLowerCase() : "";
  const extension = lower.slice(lower.lastIndexOf("/") + 1);
  const format = sourceFormatOf(extension);
  if (format) return byFormat(format);
  if (/\.json$/.test(extension)) return json();

  // Named files of an unsupported type are plain text. Empty paths and .txt
  // retain the document's declared format, as they did in the editor before.
  if (extension && !extension.endsWith(".txt")) return [];
  return byFormat(fallback);
}
