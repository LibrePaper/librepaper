// Org-mode language support: the Lezer tree of ./parser.js, for highlighting
// and folding. The parser reads the whole text at once, which is fast enough
// for Org files, and the tree is also what the document index reads.
import { indentService, Language, LanguageSupport } from "@codemirror/language";
import { countColumn, Prec } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { enclosing, orgKeymap } from "../org-editing.js";
import { pipeTableAt, pipeTableKeymap } from "../pipe-table-keymap.js";
import { parser } from "./parser.js";
import { orgData } from "./props.js";

export { parseOrg } from "./parser.js";

export const orgLanguage = new Language(orgData, parser, [], "org");

// The column where a list item's text starts, after its bullet and checkbox.
function itemTextColumn(state, item) {
  const bullet = item.firstChild;
  let end = bullet.to;
  const box = bullet.nextSibling?.name === "Checkbox" ? bullet.nextSibling : null;
  if (box) end = box.to;
  while (end < item.to && /[ \t]/.test(state.sliceDoc(end, end + 1))) end++;
  const line = state.doc.lineAt(item.from);
  return countColumn(line.text.slice(0, end - line.from), state.tabSize);
}

/**
 * The indentation of a new line in Org. Inside a list item, the text column of the item.
 * Inside a block or drawer body, the indentation of the line before. Anywhere else, null,
 * so the editor keeps its usual behaviour.
 * @param {import("@codemirror/language").IndentContext} context
 * @param {number} pos
 * @returns {number | null}
 */
function orgIndent(context, pos) {
  const { state } = context;
  const item = enclosing(state, pos, "ListItem");
  if (item) return itemTextColumn(state, item);
  if (["Block", "Drawer", "PropertyDrawer"].some((name) => enclosing(state, pos, name))) {
    const line = state.doc.lineAt(pos);
    // At the start of a line, the line before is the one whose indentation carries on.
    const before = pos === line.from && line.number > 1 ? state.doc.line(line.number - 1) : context.lineAt(pos, -1);
    return countColumn(/^[ \t]*/.exec(before.text)[0], state.tabSize);
  }
  return null;
}

/** The Org-mode language support. Table keys come first, so Tab in a table moves between cells. */
export function org() {
  return new LanguageSupport(orgLanguage, [
    Prec.high(keymap.of(pipeTableKeymap("org", pipeTableAt))),
    Prec.high(keymap.of(orgKeymap)),
    indentService.of(orgIndent),
  ]);
}
