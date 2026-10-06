import { StreamLanguage, LanguageSupport } from "@codemirror/language";
import { markdown } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { yamlFrontmatter } from "@codemirror/lang-yaml";
import { languages } from "@codemirror/language-data";
import { LanguageDescription } from "@codemirror/language";
import { stex } from "@codemirror/legacy-modes/mode/stex";
import { json } from "@codemirror/legacy-modes/mode/javascript";
import { typstLanguage } from "./typst-mode.js";

const typst = StreamLanguage.define(typstLanguage);
const latex = StreamLanguage.define(stex);
export const jsonLanguage = StreamLanguage.define(json);

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

  // @codemirror/language-data's JSON description lazily imports
  // @codemirror/lang-json, which is not installed. Return our legacy-mode
  // JSON language directly instead.
  if (name === "json") {
    return LanguageDescription.of({ name: "JSON", support: new LanguageSupport(jsonLanguage) });
  }

  return LanguageDescription.matchLanguageName(languages, name, false) || null;
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
  if (/\.json$/.test(extension)) return jsonLanguage;

  // Named files of an unsupported type are plain text. Empty paths and .txt
  // retain the document's declared format, as they did in the editor before.
  if (extension && !extension.endsWith(".txt")) return [];
  return byFormat(fallback);
}
