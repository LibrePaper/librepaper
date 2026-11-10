// Org-mode language support: the Lezer tree of ./parser.js, for highlighting
// and folding. The parser reads the whole text at once, which is fast enough
// for Org files, and the tree is also what the document index reads.
import { Language, LanguageSupport } from "@codemirror/language";
import { Prec } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { pipeTableAt, pipeTableKeymap } from "../pipe-table-keymap.js";
import { parser } from "./parser.js";
import { orgData } from "./props.js";

export { parseOrg } from "./parser.js";

export const orgLanguage = new Language(orgData, parser, [], "org");

/** The Org-mode language support. */
export function org() {
  return new LanguageSupport(orgLanguage, [Prec.high(keymap.of(pipeTableKeymap("org", pipeTableAt)))]);
}
