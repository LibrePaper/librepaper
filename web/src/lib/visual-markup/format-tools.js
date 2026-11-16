import { bindings, COMMANDS } from "../commands.js";
import { m } from "../paraglide/messages.js";

// The toolbar's tool table, kept apart from formatting.js so the top bar can
// draw the toolbar without loading CodeMirror.

// The binding a command answers to, canonical, for the tooltip to print.
const keyOf = (id) => bindings(COMMANDS.find((command) => command.id === id))[0];

// Every tool the bar shows, in the order it draws them. Consecutive tools that
// share a `group` are drawn together, with a divider before the next group.
// To add a tool, add an entry here. Exactly one of these keys says what it does:
//   mark     "bold" | "italic" | "underline" | "mono" | "inlineMath": toggles that markup on the selection
//   heading  1 | 2 | 3: makes the lines a heading of that level, or a paragraph again
//   list     "bullet" | "number": makes the lines a list, or removes the list
//   insert   an id from INSERT_ACTIONS in lib/insert.js: opens that Insert menu flow
//   command  a command id from lib/commands.js: the bar runs it through the editor, as the Edit menu does
// `icon` is a name from Icon.svelte's ICONS table, drawn on the button. `label`
// is the tooltip and accessible name. `shortcut` is the binding from
// lib/commands.js, when one exists; the tooltip prints it, and a modal editor
// mode hides it.
export const FORMAT_TOOLS = [
  { id: "bold", label: m.common_bold(), group: "emphasis", mark: "bold", icon: "bold", shortcut: keyOf("bold") },
  { id: "italic", label: m.common_italic(), group: "emphasis", mark: "italic", icon: "italic", shortcut: keyOf("italic") },
  { id: "underline", label: m.formatting_underline(), group: "emphasis", mark: "underline", icon: "underline" },
  { id: "h1", label: m.formatting_heading_1(), group: "heading", heading: 1, icon: "heading-1" },
  { id: "h2", label: m.formatting_heading_2(), group: "heading", heading: 2, icon: "heading-2" },
  { id: "h3", label: m.formatting_heading_3(), group: "heading", heading: 3, icon: "heading-3" },
  { id: "bullets", label: m.common_bulleted_list(), group: "list", list: "bullet", icon: "list" },
  { id: "numbers", label: m.common_numbered_list(), group: "list", list: "number", icon: "list-ordered" },
  { id: "mono", label: m.formatting_inline_code(), group: "code", mark: "mono", icon: "code" },
  { id: "inlineMath", label: m.common_inline_math(), group: "code", mark: "inlineMath", icon: "sigma" },
  { id: "link", label: m.common_link(), group: "insert", insert: "link", icon: "link" },
  { id: "citation", label: m.common_citation(), group: "insert", insert: "citation", icon: "quote" },
  { id: "cross-reference", label: m.common_cross_reference(), group: "insert", insert: "cross-reference", icon: "hash" },
  { id: "figure", label: m.formatting_image_or_figure(), group: "insert", insert: "figure", icon: "image" },
  { id: "table", label: m.common_table(), group: "insert", insert: "table", icon: "table" },
  { id: "comment", label: m.common_comment(), group: "comment", command: "comment", icon: "comment", shortcut: keyOf("comment") },
];

// Whether a tool is on in a state from formatState.
export function isToolActive(tool, state) {
  if (!state) return false;
  if (tool.mark) return Boolean(state[tool.mark]);
  if (tool.heading) return state.heading === tool.heading;
  if (tool.list) return state.list === tool.list;
  return false;
}
