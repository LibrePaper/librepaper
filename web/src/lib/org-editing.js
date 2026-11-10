// Org's structural editing, read from the Lezer tree of ./org-language/parser.js:
// promoting and demoting headlines and items, moving sections and items,
// adding a heading or an item, cycling a TODO keyword, toggling a checkbox and
// folding a section. Each edit is a state-level builder that returns a
// transaction spec, or null where it does not apply, so that the next binding
// gets the key. The commands are thin wrappers that dispatch the spec on a view.

import { foldable, foldedRanges, foldEffect, syntaxTree, unfoldEffect } from "@codemirror/language";

/* ------------------------------------------------------------ the tree */

/** The nearest ancestor of pos (or the node at pos) named `name`, or null. Both sides of pos are tried, so a caret at a boundary finds the node it touches. */
export function enclosing(state, pos, name) {
  const tree = syntaxTree(state);
  for (const side of [-1, 1]) {
    for (let node = tree.resolveInner(pos, side); node; node = node.parent) if (node.name === name) return node;
  }
  return null;
}

/** The first child of `node` named `name`, or null. */
function child(node, name) {
  for (let c = node.firstChild; c; c = c.nextSibling) if (c.name === name) return c;
  return null;
}

/** A headline's level, the length of its stars. */
function levelOf(headline) {
  const stars = child(headline, "Stars");
  return stars.to - stars.from;
}

/** The headline on the line of pos, or null when that line is not one. */
function headlineOn(state, pos) {
  const headline = enclosing(state, pos, "Headline");
  return headline && state.doc.lineAt(headline.from).number === state.doc.lineAt(pos).number ? headline : null;
}

/** The list item whose first line is the line of pos, or null. */
function listItemOn(state, pos) {
  const item = enclosing(state, pos, "ListItem");
  return item && state.doc.lineAt(item.from).number === state.doc.lineAt(pos).number ? item : null;
}

/* -------------------------------------------------- promote and demote */

// One star added (delta 1) or removed (delta -1) from a headline, as a change.
function starChange(headline, delta) {
  const stars = child(headline, "Stars");
  return delta > 0 ? { from: stars.from, insert: "*" } : { from: stars.from, to: stars.from + 1 };
}

// The headlines of a section, its own and its descendants'.
function headlinesIn(state, section) {
  const heads = [];
  syntaxTree(state).iterate({
    from: section.from,
    to: section.to,
    enter(node) {
      if (node.name === "Headline") heads.push(node);
    },
  });
  return heads;
}

// A headline (or the headlines of its section, for a subtree) loses a star or
// gains one. A headline never goes below level one. A list item moves two
// columns in or out.
function levelEdit(state, delta, subtree) {
  const pos = state.selection.main.head;
  const headline = headlineOn(state, pos);
  if (headline) {
    if (delta < 0 && levelOf(headline) === 1) return null;
    const heads = subtree ? headlinesIn(state, headline.parent) : [headline];
    return { changes: heads.map((h) => starChange(h, delta)) };
  }
  if (subtree) return null;
  const item = listItemOn(state, pos);
  if (!item) return null;
  const line = state.doc.lineAt(item.from);
  if (delta > 0) return { changes: { from: line.from, insert: "  " } };
  const drop = Math.min(2, /^ */.exec(line.text)[0].length);
  return drop ? { changes: { from: line.from, to: line.from + drop } } : null;
}

/* ----------------------------------------------------------- move */

// Swaps a section (or list item) with its neighbour of the same kind. The two
// blocks and the text between them trade places, so blank lines between the
// blocks stay where they are, and the text outside the two blocks is untouched.
function moveEdit(state, up) {
  const pos = state.selection.main.head;
  const headline = headlineOn(state, pos);
  const node = headline ? headline.parent : listItemOn(state, pos);
  if (!node) return null;
  const neighbour = up ? node.prevSibling : node.nextSibling;
  if (!neighbour || neighbour.name !== node.name) return null;
  if (headline && levelOf(child(neighbour, "Headline")) !== levelOf(headline)) return null;
  const [first, second] = up ? [neighbour, node] : [node, neighbour];
  const { doc } = state;
  const gapLength = second.from - first.to;
  const insert = doc.sliceString(second.from, second.to) + doc.sliceString(first.to, second.from) + doc.sliceString(first.from, first.to);
  // Where the moved block now starts: at the front when it moved up, after the other block when it moved down.
  const start = up ? first.from : first.from + (second.to - second.from) + gapLength;
  const cursor = start + Math.max(0, pos - node.from);
  return { changes: { from: first.from, to: second.to, insert }, selection: { anchor: cursor } };
}

/* ------------------------------------------------------- new heading */

// A new headline after the current section, at its level, or a new item after
// the current item. The item keeps its bullet (a number goes up by one) and
// its checkbox, which starts empty.
function newHeadingEdit(state) {
  const pos = state.selection.main.head;
  const item = enclosing(state, pos, "ListItem");
  if (item) {
    const line = state.doc.lineAt(item.from);
    const lead = line.text.slice(0, item.from - line.from);
    const bullet = state.sliceDoc(child(item, "ListBullet").from, child(item, "ListBullet").to);
    const numbered = /^(\d+)([.)])$/.exec(bullet);
    const next = numbered ? `${Number(numbered[1]) + 1}${numbered[2]}` : bullet;
    const box = child(item, "Checkbox") ? "[ ] " : "";
    const insert = "\n" + lead + next + " " + box;
    return { changes: { from: item.to, insert }, selection: { anchor: item.to + insert.length } };
  }
  const section = enclosing(state, pos, "Section");
  if (!section) return null;
  const insert = "\n" + "*".repeat(levelOf(child(section, "Headline"))) + " ";
  return { changes: { from: section.to, insert }, selection: { anchor: section.to + insert.length } };
}

/* ----------------------------------------------------- TODO keywords */

const DEFAULT_TODO = ["TODO", "DONE"];
const TODO_KEYWORD = /^(TODO|SEQ_TODO|TYP_TODO)$/;

/** The words the #+TODO, #+SEQ_TODO and #+TYP_TODO lines declare, in order, without their fast-key suffixes and the done separator. */
function todoWords(state) {
  const words = [];
  syntaxTree(state).iterate({
    enter(node) {
      if (node.name !== "Keyword") return;
      const key = child(node, "KeywordKey");
      const value = child(node, "KeywordValue");
      if (!key || !value) return;
      if (!TODO_KEYWORD.test(state.sliceDoc(key.from, key.to).split("[")[0].trim().toUpperCase())) return;
      for (const word of state.sliceDoc(value.from, value.to).split(/\s+/)) {
        const name = word.replace(/\(.*$/, "");
        if (name && name !== "|" && !words.includes(name)) words.push(name);
      }
    },
  });
  return words.length ? words : DEFAULT_TODO;
}

// The keyword moves one step along the sequence, with "no keyword" as the
// first and last state. Only the stars, the keyword and the space before the
// title are in reach, so the title is never changed by this.
function todoEdit(state, forward) {
  const { main } = state.selection;
  if (!main.empty) return null;
  const headline = headlineOn(state, main.head);
  if (!headline) return null;
  const stars = child(headline, "Stars");
  const title = child(headline, "Title");
  if (main.head > (title ? title.from : headline.to)) return null;
  const cycle = ["", ...todoWords(state)];
  const todo = child(headline, "TodoKeyword");
  const current = todo ? state.sliceDoc(todo.from, todo.to) : "";
  const i = Math.max(0, cycle.indexOf(current));
  const next = cycle[(i + (forward ? 1 : cycle.length - 1)) % cycle.length];
  if (!todo) return { changes: { from: stars.to, insert: " " + next } };
  if (next) return { changes: { from: todo.from, to: todo.to, insert: next } };
  // The space after the keyword goes with it, so "* TODO Title" becomes "* Title".
  const end = state.sliceDoc(todo.to, todo.to + 1) === " " ? todo.to + 1 : todo.to;
  return { changes: { from: todo.from, to: end } };
}

/* ------------------------------------------------- checkbox and fold */

// [ ] and [-] become [X]; [X] becomes [ ].
function checkboxEdit(state) {
  const item = listItemOn(state, state.selection.main.head);
  const box = item && child(item, "Checkbox");
  if (!box) return null;
  const text = state.sliceDoc(box.from, box.to);
  const insert = text === "[ ]" || text === "[-]" ? "[X]" : "[ ]";
  return { changes: { from: box.from, to: box.to, insert } };
}

// A headline line folds its section under it, or unfolds it when it is folded.
function foldEdit(state) {
  const { main } = state.selection;
  if (!main.empty || !headlineOn(state, main.head)) return null;
  const line = state.doc.lineAt(main.head);
  const range = foldable(state, line.from, line.to);
  if (!range || state.doc.lineAt(range.from).number !== line.number) return null;
  let folded = false;
  foldedRanges(state).between(range.from, range.to, (from) => {
    if (from === range.from) folded = true;
  });
  return { effects: folded ? unfoldEffect.of(range) : foldEffect.of(range) };
}

/* ------------------------------------------------------- the commands */

// A command runs a builder on the view and reports whether it applied.
const command = (build) => (view) => {
  const spec = build(view.state);
  if (!spec) return false;
  view.dispatch(spec);
  return true;
};

export const promoteHeading = command((state) => levelEdit(state, -1, false));
export const demoteHeading = command((state) => levelEdit(state, 1, false));
export const promoteSubtree = command((state) => levelEdit(state, -1, true));
export const demoteSubtree = command((state) => levelEdit(state, 1, true));
export const moveUp = command((state) => moveEdit(state, true));
export const moveDown = command((state) => moveEdit(state, false));
export const newHeading = command(newHeadingEdit);
export const todoNext = command((state) => todoEdit(state, true));
export const todoPrevious = command((state) => todoEdit(state, false));
export const toggleCheckbox = command(checkboxEdit);
export const foldCycle = command(foldEdit);

/** The Org commands by the ids lib/commands.js lists them under. */
export const ORG_COMMANDS = {
  "org-promote": promoteHeading,
  "org-demote": demoteHeading,
  "org-promote-subtree": promoteSubtree,
  "org-demote-subtree": demoteSubtree,
  "org-move-up": moveUp,
  "org-move-down": moveDown,
  "org-new-heading": newHeading,
  "org-todo-next": todoNext,
  "org-todo-previous": todoPrevious,
  "org-toggle-checkbox": toggleCheckbox,
  "org-fold": foldCycle,
};

/** The key bindings of the Org language, highest precedence. Each returns false where it does not apply, so the rest of the keymap runs. */
export const orgKeymap = [
  { key: "Alt-ArrowLeft", run: promoteHeading },
  { key: "Alt-ArrowRight", run: demoteHeading },
  { key: "Alt-Shift-ArrowLeft", run: promoteSubtree },
  { key: "Alt-Shift-ArrowRight", run: demoteSubtree },
  { key: "Alt-ArrowUp", run: moveUp },
  { key: "Alt-ArrowDown", run: moveDown },
  { key: "Alt-Enter", run: newHeading },
  { key: "Shift-ArrowRight", run: todoNext },
  { key: "Shift-ArrowLeft", run: todoPrevious },
  { key: "Tab", run: foldCycle },
];
