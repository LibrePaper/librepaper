// Org's structural editing, run through the commands against plain EditorStates.
// A stand-in view keeps the state each command leaves, so the commands are the
// ones the editor binds, and nothing here needs a DOM.
import assert from "node:assert/strict";
import { ensureSyntaxTree, foldedRanges, foldState, getIndentation } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { org } from "../../src/lib/org-language/index.js";
import {
  continueList, demoteHeading, demoteSubtree, foldCycle, moveDown, moveUp, newHeading, ORG_COMMANDS, orgKeymap,
  promoteHeading, promoteSubtree, toggleCheckbox, todoNext, todoPrevious,
} from "../../src/lib/org-editing.js";

/* ------------------------------------------------------------- the helpers */

// The tree is read in full, as the editor reads a small file, before a command runs.
function ready(state) {
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

// A state over `doc`, the selection from `anchor` to `head` (anchor defaults to head).
function stateOf(doc, head = 0, anchor = head) {
  return ready(EditorState.create({ doc, selection: { anchor, head }, extensions: [org(), foldState] }));
}

// Runs a command on a stand-in view. Says whether it applied, and what the document and caret are after.
function run(command, state) {
  const view = {
    state,
    dispatch(spec) {
      this.state = ready(this.state.update(spec).state);
    },
  };
  const applied = command(view);
  return { applied, state: view.state, doc: view.state.doc.toString(), head: view.state.selection.main.head };
}

// How many folds the state holds.
function folds(state) {
  let count = 0;
  foldedRanges(state).between(0, state.doc.length, () => {
    count++;
  });
  return count;
}

/* ------------------------------------------------------ promote and demote */

{
  const doc = "* One\nbody\n** Two\ntext\n* Three\n";
  const two = doc.indexOf("** Two") + 3;
  let r = run(promoteHeading, stateOf(doc, two));
  assert.equal(r.applied, true);
  assert.equal(r.doc, "* One\nbody\n* Two\ntext\n* Three\n");

  r = run(demoteHeading, stateOf(doc, 0));
  assert.equal(r.doc, "** One\nbody\n** Two\ntext\n* Three\n");

  assert.equal(run(promoteHeading, stateOf(doc, 0)).applied, false, "a level-one headline does not go below one");
  assert.equal(run(promoteHeading, stateOf(doc, doc.indexOf("body"))).applied, false, "a paragraph is not a headline");
}

// A list item moves two columns in or out, and only an unindented item can't go further out.
{
  let r = run(demoteHeading, stateOf("- a\n- b\n", 6));
  assert.equal(r.doc, "- a\n  - b\n");
  r = run(promoteHeading, stateOf("- a\n  - b\n", 8));
  assert.equal(r.doc, "- a\n- b\n");
  assert.equal(run(promoteHeading, stateOf("- a\n- b\n", 6)).applied, false, "an unindented item stays where it is");
}

/* ------------------------------------------------------------- subtrees */

{
  const doc = "* Top\n** Parent\n*** Kid\n** Sib\n";
  const parent = doc.indexOf("** Parent");
  assert.equal(run(promoteSubtree, stateOf(doc, 0)).applied, false, "a level-one root has nowhere to go");
  assert.equal(run(promoteSubtree, stateOf(doc, parent)).doc, "* Top\n* Parent\n** Kid\n** Sib\n", "the subtree moves, the sibling does not");
  assert.equal(run(demoteSubtree, stateOf(doc, parent)).doc, "* Top\n*** Parent\n**** Kid\n** Sib\n");
  assert.equal(run(demoteSubtree, stateOf("text\n", 0)).applied, false, "demote subtree needs a headline");
}

/* ------------------------------------------------------------------ move */

{
  const doc = "* A\na\n* B\nb\n";
  let r = run(moveDown, stateOf(doc, 0));
  assert.equal(r.doc, "* B\nb\n* A\na\n");
  assert.equal(r.head, 6, "the caret stays on the moved headline");

  r = run(moveUp, stateOf(doc, 6));
  assert.equal(r.doc, "* B\nb\n* A\na\n");
  assert.equal(r.head, 0);

  assert.equal(run(moveUp, stateOf(doc, 0)).applied, false, "the first section has no sibling above");
  assert.equal(run(moveDown, stateOf("* A\na\n* B\nb", 0)).doc, "* B\nb\n* A\na", "a document with no final newline keeps none");
}

{
  // X is level three and B is level two; both are children of A, and a level-three sibling is not a neighbour.
  const doc = "* A\n*** X\n** B\n";
  assert.equal(run(moveDown, stateOf(doc, doc.indexOf("*** X"))).applied, false, "the next sibling is a different level");
}

{
  const items = "- a\n- b\n- c\n";
  let r = run(moveDown, stateOf(items, 0));
  assert.equal(r.doc, "- b\n- a\n- c\n");
  assert.equal(r.head, 4);
  r = run(moveUp, stateOf(items, 8));
  assert.equal(r.doc, "- a\n- c\n- b\n");
  assert.equal(run(moveUp, stateOf("- a\n", 0)).applied, false, "the first item has no sibling above");
}

/* ----------------------------------------------------------- new heading */

{
  const doc = "* A\nbody\n* B\n";
  const r = run(newHeading, stateOf(doc, doc.indexOf("body") + 2));
  assert.equal(r.doc, "* A\nbody\n* \n* B\n", "a headline at the section's level after the section");
  assert.equal(r.head, 11, "the caret is after the stars and space");

  const nested = "* A\n** B\ntext";
  const s = run(newHeading, stateOf(nested, nested.indexOf("text")));
  assert.equal(s.doc, "* A\n** B\ntext\n** ", "inside a subsection, the new headline is at its level");
  assert.equal(s.head, 17);

  assert.equal(run(newHeading, stateOf("text\n* A\n", 0)).applied, false, "before the first headline there is no section");
}

{
  const items = "1. one\n2. two\n";
  const r = run(newHeading, stateOf(items, 4));
  assert.equal(r.doc, "1. one\n2. \n2. two\n", "inside an item, a new item after it, numbered on");
  assert.equal(r.head, 10);

  const box = "- [ ] task\n";
  const b = run(newHeading, stateOf(box, box.indexOf("task") + 1));
  assert.equal(b.doc, "- [ ] task\n- [ ] \n", "a checkbox item comes back with an empty box");
}

/* ----------------------------------------------------------- TODO cycle */

{
  let s = stateOf("* Task\n", 0);
  const seen = [];
  for (const step of [todoNext, todoNext, todoNext, todoPrevious]) {
    s = run(step, s).state;
    seen.push(s.doc.toString());
  }
  assert.deepEqual(seen, ["* TODO Task\n", "* DONE Task\n", "* Task\n", "* DONE Task\n"], "none, TODO, DONE, none, and back");
}

{
  const custom = "#+TODO: NEXT(n) | DONE(d) WAIT(w)\n* Task\n";
  let s = stateOf(custom, custom.indexOf("* Task"));
  const seen = [];
  for (let i = 0; i < 4; i++) {
    s = run(todoNext, s).state;
    seen.push(s.doc.toString().split("\n")[1]);
  }
  assert.deepEqual(seen, ["* NEXT Task", "* DONE Task", "* WAIT Task", "* Task"], "the sequence comes from the #+TODO line, its fast keys dropped");
}

{
  assert.equal(run(todoNext, stateOf("* TODO Task\n", "* TODO Task".indexOf("ask"))).applied, false, "in the title, the arrow selects");
  assert.equal(run(todoNext, stateOf("* Task\n", 0, 2)).applied, false, "a selection is not a keyword change");
  assert.equal(run(todoNext, stateOf("text\n", 0)).applied, false, "a line that is not a headline");
}

/* ---------------------------------------------------------- checkboxes */

{
  const boxes = "- [ ] a\n- [X] b\n- [-] c\n";
  assert.equal(run(toggleCheckbox, stateOf(boxes, 2)).doc, "- [X] a\n- [X] b\n- [-] c\n");
  assert.equal(run(toggleCheckbox, stateOf(boxes, 10)).doc, "- [ ] a\n- [ ] b\n- [-] c\n");
  assert.equal(run(toggleCheckbox, stateOf(boxes, 18)).doc, "- [ ] a\n- [X] b\n- [X] c\n", "a partial box becomes done");
  assert.equal(run(toggleCheckbox, stateOf("- plain\n", 3)).applied, false, "an item without a box has nothing to toggle");
}

/* ------------------------------------------------------------ fold cycle */

{
  const doc = "* A\nbody\n* B\n";
  let r = run(foldCycle, stateOf(doc, 0));
  assert.equal(r.applied, true);
  assert.equal(folds(r.state), 1, "the section folds under its headline");
  r = run(foldCycle, r.state);
  assert.equal(folds(r.state), 0, "a folded section unfolds");
  assert.equal(run(foldCycle, stateOf(doc, 6)).applied, false, "a body line is not a headline");
  assert.equal(run(foldCycle, stateOf(doc, 0, 2)).applied, false, "a selection leaves Tab to the rest");
}

/* ---------------------------------------------------------- indentation */

{
  assert.equal(getIndentation(stateOf("- [ ] one\n", 6), 6), 6, "Enter in an item indents to its text, after the box");
  assert.equal(getIndentation(stateOf("- a\n  - b\n", 8), 8), 4, "a nested item's text column counts its indentation");
  assert.equal(getIndentation(stateOf("text\n  more\n", 5), 5), null, "a paragraph leaves indentation to the editor");

  const block = "#+begin_quote\n    deep\n  more\n#+end_quote\n";
  assert.equal(getIndentation(stateOf(block, block.indexOf("  more")), block.indexOf("  more")), 4, "a block body keeps the line before's indentation");
}

/* ------------------------------------------------------- list continuation */

{
  // Enter after an item's text starts the next item with the same bullet.
  let r = run(continueList, stateOf("- a\n", 3));
  assert.equal(r.applied, true);
  assert.equal(r.doc, "- a\n- \n", "a dash item continues with a dash");
  assert.equal(r.head, 6, "the caret is after the new bullet");
  assert.equal(run(continueList, stateOf("+ a\n", 3)).doc, "+ a\n+ \n", "a plus item continues with a plus");
  assert.equal(run(continueList, stateOf("1. one\n", 6)).doc, "1. one\n2. \n", "a number goes up by one");
  assert.equal(run(continueList, stateOf("3) x\n", 4)).doc, "3) x\n4) \n", "a parenthesis number keeps its closer");
}

{
  // A checkbox comes back empty, and the new item's text starts after it.
  const box = "- [ ] task\n";
  const r = run(continueList, stateOf(box, 10));
  assert.equal(r.doc, "- [ ] task\n- [ ] \n", "an item with a box continues with an empty box");
  assert.equal(r.head, 17);
}

{
  // A nested item keeps its indentation.
  const doc = "- a\n  - b\n";
  const r = run(continueList, stateOf(doc, doc.indexOf("b") + 1));
  assert.equal(r.doc, "- a\n  - b\n  - \n", "the new item is at the nested item's indentation");
  assert.equal(r.head, 14);
}

{
  // The text after the caret moves onto the new item, and the blanks at the split go.
  const r = run(continueList, stateOf("- hello world\n", 7));
  assert.equal(r.doc, "- hello\n- world\n", "the split text starts the new item");
  assert.equal(r.head, 10);
}

{
  // An item with no text ends the list: the bullet and checkbox go, the indentation stays.
  let r = run(continueList, stateOf("- a\n- ", 6));
  assert.equal(r.applied, true);
  assert.equal(r.doc, "- a\n", "an empty item leaves an empty line");
  assert.equal(r.head, 4);

  r = run(continueList, stateOf("  - [ ]\nnext", 7));
  assert.equal(r.doc, "  \nnext", "an empty box item keeps its indentation and loses its box");
  assert.equal(r.head, 2);
}

{
  // A description item continues with its bullet alone.
  const doc = "- term :: text\n";
  const r = run(continueList, stateOf(doc, 14));
  assert.equal(r.doc, "- term :: text\n- \n", "a description item continues with just its bullet");
  assert.equal(r.head, 17);
}

{
  // Outside a list item, and before its bullet, Enter is left to the rest of the keymap.
  assert.equal(run(continueList, stateOf("- a\n", 0)).applied, false, "the caret in the bullet is not in the text");
  assert.equal(run(continueList, stateOf("* Heading\n", 4)).applied, false, "a headline is not a list item");
  assert.equal(run(continueList, stateOf("text\n", 2)).applied, false, "a paragraph is not a list item");
  assert.equal(run(continueList, stateOf("| a | b |\n", 3)).applied, false, "a table row is not a list item");
  const src = "#+begin_src org\n- item\n#+end_src\n";
  assert.equal(run(continueList, stateOf(src, src.indexOf("item") + 2)).applied, false, "a source block line is not a list item");
  assert.equal(run(continueList, stateOf("- a\n", 3, 1)).applied, false, "a selection is left to the editor");
}

/* ------------------------------------------------------------- the keys */

{
  const keys = orgKeymap.map((binding) => binding.key);
  for (const key of ["Alt-ArrowLeft", "Alt-ArrowRight", "Alt-Shift-ArrowLeft", "Alt-Shift-ArrowRight", "Alt-ArrowUp", "Alt-ArrowDown", "Alt-Enter", "Enter", "Shift-ArrowLeft", "Shift-ArrowRight", "Tab"]) {
    assert.ok(keys.includes(key), `${key} is bound`);
  }
  assert.equal(Object.keys(ORG_COMMANDS).length, 11, "every Org command has an id the palette can run");
  for (const command of Object.values(ORG_COMMANDS)) assert.equal(typeof command, "function");
}
