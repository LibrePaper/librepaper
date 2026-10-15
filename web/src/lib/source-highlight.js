import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

// Shared by every source editor so the same language construct has the same
// visual meaning in LaTeX, Markdown, Typst, HTML, and Quarto files.
export const sourceHighlighting = syntaxHighlighting(HighlightStyle.define([
  { tag: [tags.keyword, tags.operatorKeyword, tags.controlKeyword, tags.definitionKeyword, tags.moduleKeyword], color: "var(--source-highlight-keyword)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], color: "var(--source-highlight-string)" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.constant(tags.variableName), tags.standard(tags.variableName)], color: "var(--source-highlight-constant)" },
  { tag: [tags.comment, tags.lineComment, tags.blockComment], color: "var(--source-highlight-comment)", fontStyle: "italic" },
  { tag: tags.tagName, color: "var(--source-highlight-tag)" },
  { tag: [tags.attributeName, tags.propertyName], color: "var(--source-highlight-property)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.name), tags.function(tags.propertyName)], color: "var(--source-highlight-function)" },
  { tag: [tags.definition(tags.variableName), tags.self, tags.typeName, tags.className, tags.namespace], color: "var(--source-highlight-variable)" },
  { tag: [tags.heading, tags.heading1, tags.heading2, tags.heading3, tags.heading4, tags.heading5, tags.heading6], color: "var(--source-highlight-heading)", fontWeight: "600" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: [tags.link, tags.url], color: "var(--source-highlight-link)", textDecoration: "underline" },
  { tag: tags.labelName, color: "var(--source-highlight-function)" },
  { tag: tags.monospace, color: "var(--source-highlight-monospace)" },
  { tag: [tags.punctuation, tags.separator, tags.angleBracket, tags.bracket], color: "var(--source-highlight-punctuation)" },
  { tag: [tags.meta, tags.processingInstruction, tags.documentMeta], color: "var(--source-highlight-meta)" },
  { tag: tags.invalid, color: "var(--source-highlight-invalid)", textDecoration: "underline wavy" },
]));
