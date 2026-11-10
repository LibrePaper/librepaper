// BibTeX language support: the Lezer grammar (bibtex.grammar, built into
// parser.js by tools/bibtex-grammar.mjs) plus highlighting and folding.
import { LRLanguage, LanguageSupport, foldNodeProp } from "@codemirror/language";
import { styleTags, tags } from "@lezer/highlight";
import { parser } from "./parser.js";

/** Fold an entry from the end of its first line to just before its closing brace or paren. */
function foldEntry(node, state) {
  const from = state.doc.lineAt(node.from).to;
  const last = state.sliceDoc(node.to - 1, node.to);
  const to = last === "}" || last === ")" ? node.to - 1 : node.to;
  if (from >= to) return null;
  return state.doc.lineAt(from).number < state.doc.lineAt(to).number ? { from, to } : null;
}

export const bibtexLanguage = LRLanguage.define({
  name: "bibtex",
  parser: parser.configure({
    props: [
      styleTags({
        At: tags.meta,
        "EntryType StringKw PreambleKw CommentKw": tags.keyword,
        CitationKey: tags.labelName,
        FieldName: tags.propertyName,
        "BracedString QuotedString": tags.string,
        Number: tags.number,
        Variable: tags.variableName,
        "LineComment Junk CommentText": tags.comment,
        "OpenBrace CloseBrace OpenParen CloseParen": tags.brace,
        "Equals Hash": tags.operator,
        Comma: tags.separator,
      }),
      foldNodeProp.add({
        "Entry StringEntry PreambleEntry CommentEntry": foldEntry,
      }),
    ],
  }),
  languageData: {
    commentTokens: { line: "%" },
    closeBrackets: { brackets: ["{", "\"", "("] },
  },
});

/** BibTeX language support. */
export function bibtex() {
  return new LanguageSupport(bibtexLanguage);
}
