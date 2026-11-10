// Org-mode language support: the Lezer tree of org.wasm, for highlighting and
// folding. The module parses the whole text at once, reusing the top-level
// sections an edit did not touch, and the tree is also what the document index
// reads. Until the module has loaded the editor shows plain text (the keymaps
// work meanwhile); the view picks the language up when it arrives.
import { indentService, Language, LanguageSupport } from "@codemirror/language";
import { Compartment, countColumn, EditorState, Prec } from "@codemirror/state";
import { keymap, ViewPlugin } from "@codemirror/view";
import { Parser } from "@lezer/common";
import { enclosing, orgKeymap } from "../org-editing.js";
import { pipeTableAt, pipeTableKeymap } from "../pipe-table-keymap.js";
import { loadOrgSyntax, orgSyntax } from "./module.js";
import { ORG_LANGUAGE_DATA, orgData } from "./props.js";

class OrgParser extends Parser {
  constructor(api) {
    super();
    this.api = api;
  }

  createParse(input, fragments) {
    const api = this.api;
    let done = false;
    return {
      parsedPos: 0,
      stoppedAt: null,
      stopAt() {},
      advance() {
        if (done) return null;
        done = true;
        const tree = api.parse(input.read(0, input.length), fragments);
        this.parsedPos = input.length;
        return tree;
      },
    };
  }
}

const languages = new WeakMap();

/** The Org language for a loaded module. */
export function orgLanguageFor(api) {
  let language = languages.get(api);
  if (!language) {
    language = new Language(orgData, new OrgParser(api), [], "org");
    languages.set(api, language);
  }
  return language;
}

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

/** The keys and indentation, which work with or without the tree. */
function editing() {
  return [
    Prec.high(keymap.of(pipeTableKeymap("org", pipeTableAt))),
    Prec.high(keymap.of(orgKeymap)),
    indentService.of(orgIndent),
  ];
}

/** The Org-mode language support. Table keys come first, so Tab in a table moves between cells. */
export function org() {
  // Already loaded: no plain-text paint first.
  const loaded = orgSyntax();
  if (loaded) return new LanguageSupport(orgLanguageFor(loaded), editing());
  const compartment = new Compartment();
  const upgrade = ViewPlugin.define((view) => {
    let alive = true;
    loadOrgSyntax().then((api) => {
      if (api && alive) view.dispatch({ effects: compartment.reconfigure(new LanguageSupport(orgLanguageFor(api))) });
    });
    return { destroy() { alive = false; } };
  });
  // Until the language arrives, its data (comment marker, bracket pairs) stands in; the language replaces it.
  return [compartment.of(EditorState.languageData.of(() => [ORG_LANGUAGE_DATA])), editing(), upgrade];
}
