// What an Org syntax tree needs to highlight and fold: node props attached by
// node name. The colours follow the old stream mode (headline text is a
// heading, keyword lines and blocks are meta, and so on). A rule with `/...`
// is inherited by the node's children; one without styles only the text of
// the node that no child covers.
import { defineLanguageFacet, foldNodeProp, languageDataProp } from "@codemirror/language";
import { styleTags, tags } from "@lezer/highlight";

export const orgData = defineLanguageFacet({
  commentTokens: { line: "# " },
  // No apostrophe (prose), no markup characters (they collide with lists, headlines and paths).
  closeBrackets: { brackets: ["(", "[", "{", "\""] },
});

const highlighting = styleTags({
  "Stars Title/...": tags.heading,
  TodoKeyword: tags.keyword,
  Priority: tags.atom,
  "Tags/...": tags.tagName,
  "PlanningKeyword ClockKeyword": tags.keyword,
  "TimestampActive TimestampInactive": tags.number,
  "Drawer/... PropertyDrawer/...": tags.meta,
  "Keyword KeywordKey TableFormula": tags.meta,
  KeywordValue: tags.string,
  "BlockBegin/... BlockEnd/...": tags.meta,
  VerbatimBody: tags.monospace,
  CommentBody: tags.comment,
  LatexEnvironment: tags.string,
  "LatexEnvBegin/... LatexEnvEnd/...": tags.keyword,
  Comment: tags.comment,
  FixedWidth: tags.monospace,
  HorizontalRule: tags.contentSeparator,
  "TableDelimiter TableRule": tags.punctuation,
  ListBullet: tags.punctuation,
  Checkbox: tags.atom,
  "FootnoteLabel FootnoteReference/... Citation/... Target RadioTarget": tags.labelName,
  "Bold/...": tags.strong,
  "Italic/... Underline/...": tags.emphasis,
  "StrikeThrough/...": tags.strikethrough,
  "Verbatim/... Code/... InlineSrc/... InlineBabelCall/...": tags.monospace,
  "Link/...": tags.link,
  "Macro/...": tags.processingInstruction,
  "InlineMath DisplayMath": tags.string,
  "Entity LatexCommand": tags.keyword,
});

const multiline = (state, from, to) => from < to && state.doc.lineAt(from).number < state.doc.lineAt(to).number;

/** Fold from the end of the node's first line to the start of the line its last child opens. */
function foldToLastLine(node, state) {
  const last = node.lastChild;
  // A block still being typed has no end line: fold to its last content.
  if (node.name === "Block" && last && last.name !== "BlockEnd") {
    const from = state.doc.lineAt(node.from).to;
    return node.to > from ? { from, to: node.to } : null;
  }
  if (!last || last.from === node.firstChild?.from) return null;
  const from = state.doc.lineAt(node.from).to;
  const to = state.doc.lineAt(last.from).from - 1;
  return multiline(state, from, to + 1) && to > from ? { from, to } : null;
}

/** A section folds under its headline, to the end of its last content. */
function foldSection(node, state) {
  const headline = node.firstChild;
  if (!headline) return null;
  const from = state.doc.lineAt(headline.from).to;
  return node.to > from ? { from, to: node.to } : null;
}

const folding = foldNodeProp.add({
  Section: foldSection,
  "Block Drawer PropertyDrawer LatexEnvironment": foldToLastLine,
});

/** The node props every Org node type is defined with. */
export const nodeProps = [highlighting, folding, languageDataProp.add({ Document: orgData })];
