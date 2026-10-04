import { StreamLanguage } from "@codemirror/language";
import { markdown } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { yamlFrontmatter } from "@codemirror/lang-yaml";
import { languages } from "@codemirror/language-data";
import { LanguageDescription } from "@codemirror/language";
import { stex } from "@codemirror/legacy-modes/mode/stex";
import { typstLanguage } from "./typst-mode.js";

const typst = StreamLanguage.define(typstLanguage);
const latex = StreamLanguage.define(stex);

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
    if (close < 0) return null;
    name = name.slice(1, close).trim();
  }
  name = name.split(/[\s,]+/, 1)[0]?.toLowerCase() || "";
  if (!name) return null;
  return LanguageDescription.matchLanguageName(languages, name) || null;
}

const markdownLanguage = () => yamlFrontmatter({
  content: markdown({ codeLanguages: codeLanguage }),
});

function byFormat(format) {
  switch ((format || "").toLowerCase()) {
    case "latex":
    case "tex": return latex;
    case "typst": return typst;
    case "html": return html();
    case "quarto":
    case "markdown":
    case "md": return markdownLanguage();
    default: return [];
  }
}

/** Select a CodeMirror language from a file path, falling back for unnamed text. */
export function sourceLanguage(path, fallback = "") {
  const lower = typeof path === "string" ? path.toLowerCase() : "";
  const extension = lower.slice(lower.lastIndexOf("/") + 1);
  if (/\.(?:tex|ltx|sty|cls)$/.test(extension)) return latex;
  if (/\.typ$/.test(extension)) return typst;
  if (/\.(?:md|markdown|qmd)$/.test(extension)) return markdownLanguage();
  if (/\.(?:html|htm)$/.test(extension)) return html();

  // Named files of an unsupported type are plain text. Empty paths and .txt
  // retain the document's declared format, as they did in the editor before.
  if (extension && !extension.endsWith(".txt")) return [];
  return byFormat(fallback);
}
