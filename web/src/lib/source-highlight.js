import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

// Shared by every source editor so the same language construct has the same
// visual meaning in LaTeX, Markdown, Typst, HTML, and Quarto files.
export const sourceHighlighting = syntaxHighlighting(HighlightStyle.define([
  { tag: tags.keyword, color: "var(--source-highlight-keyword)", fontWeight: "600" },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--source-highlight-string)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--source-highlight-number)" },
  { tag: tags.comment, color: "var(--source-highlight-comment)", fontStyle: "italic" },
  { tag: [tags.tagName, tags.angleBracket], color: "var(--source-highlight-tag)" },
  { tag: [tags.attributeName, tags.propertyName], color: "var(--source-highlight-property)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.name)], color: "var(--source-highlight-function)" },
  { tag: [tags.heading, tags.heading1, tags.heading2, tags.heading3], color: "var(--source-highlight-heading)", fontWeight: "600" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: [tags.link, tags.url, tags.labelName], color: "var(--source-highlight-link)", textDecoration: "underline" },
  { tag: tags.monospace, color: "var(--source-highlight-monospace)" },
  { tag: [tags.punctuation, tags.separator], color: "var(--source-highlight-punctuation)" },
  { tag: tags.invalid, color: "var(--source-highlight-invalid)", textDecoration: "underline wavy" },
]));
