// The one list of what the workspace can be asked to do, and the rules that
// decide whether a keystroke is asking for it.
//
// What is checked here is the contract the rest of the feature rests on: the
// keyboard, the menus, the palette and the help table all read this module, so
// a shortcut that collides with the browser, a chord no Mac can press, or a
// help row naming a key nothing listens for is caught before any of the four
// draws it.
import assert from "node:assert/strict";
import {
  COMMANDS, RESERVED, availability, bindings, caps, commandFor, filter, fromEvent, helpTable,
  label, menuKeys, normalize, offered, reserved, typeable,
} from "../../src/lib/commands.js";

/* ------------------------------------------------------------- the notation */

// One chord, one spelling. The canonical form is what a duplicate binding
// would collide on, so it has to be reached the same way from every way of
// writing the same keystroke.
assert.equal(normalize("Mod+Alt+P"), "mod+alt+p");
assert.equal(normalize("mod+alt+p"), "mod+alt+p");
assert.equal(normalize("Alt+Mod+P"), "mod+alt+p", "the order the modifiers were written in is not part of the chord");
assert.equal(normalize("Mod+\\"), "mod+\\");
assert.equal(normalize("Mod+,"), "mod+,");

// Shift is part of a letter chord and not part of a punctuation one. `?` is
// Shift+/ on most keyboards and its own key on some, and a binding that
// insisted on the modifier would be a binding that worked on one of them.
assert.equal(normalize("Mod+Shift+Z"), "mod+shift+z");
assert.equal(normalize("?"), "?");
assert.equal(normalize("Shift+?"), "?", "punctuation does not carry the Shift that typed it");

/* --------------------------------------------------------- reading an event */

const press = (key, extra = {}) => ({ key, code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...extra });

// Mod is the one difference between the platforms, in both directions: the
// modifier that is not Mod here disqualifies the event rather than being
// ignored, so a chord the window manager sent is not mistaken for ours.
assert.equal(fromEvent(press("p", { ctrlKey: true, altKey: true }), false), "mod+alt+p");
assert.equal(fromEvent(press("p", { metaKey: true, altKey: true }), true), "mod+alt+p");
assert.equal(fromEvent(press("p", { ctrlKey: true, altKey: true }), true), "", "Ctrl on a Mac is not ⌘");
assert.equal(fromEvent(press("p", { metaKey: true, altKey: true }), false), "", "the Windows key is not Ctrl");

// A modifier pressed on its own is not a chord.
for (const key of ["Control", "Meta", "Alt", "Shift"]) assert.equal(fromEvent(press(key), false), "");

// Alt is a compose key on macOS: ⌥P types "π" and ⌥/ types "÷". Read from the
// character alone, every command in the Mod+Alt namespace would be a command
// no Mac could press, so an Alt chord is read from the physical key.
assert.equal(fromEvent(press("π", { metaKey: true, altKey: true, code: "KeyP" }), true), "mod+alt+p");
assert.equal(fromEvent(press("÷", { metaKey: true, altKey: true, code: "Slash" }), true), "mod+alt+/");
assert.equal(fromEvent(press("¬", { metaKey: true, altKey: true, code: "KeyL" }), true), "mod+alt+l");
// Without Alt the character is the truth, whatever key produced it.
assert.equal(fromEvent(press("\\", { ctrlKey: true, code: "IntlBackslash" }), false), "mod+\\");

// `?` arrives as itself, with the Shift that typed it left out, so it matches
// the binding however the keyboard produces it.
assert.equal(fromEvent(press("?", { shiftKey: true }), false), "?");

/* -------------------------------------------------------------- the keycaps */

assert.deepEqual(caps("Mod+Alt+P", true), ["⌘", "⌥", "P"]);
assert.deepEqual(caps("Mod+Alt+P", false), ["Ctrl", "Alt", "P"]);
assert.deepEqual(caps("Mod+\\", false), ["Ctrl", "\\"]);
assert.equal(label("Mod+Shift+Z", true), "⌘⇧Z", "a Mac menu prints the caps run together");
assert.equal(label("Mod+Shift+Z", false), "Ctrl+Shift+Z");

/* -------------------------------------------------- what the browser keeps */

// The reason this list exists: a command bound to a browser shortcut is a
// command that works until somebody presses it in a second browser.
for (const command of COMMANDS) {
  for (const binding of [...bindings(command, true), ...bindings(command, false)]) {
    assert.ok(!reserved(binding), `${command.id} claims ${binding}, which the browser answers`);
  }
}
assert.ok(reserved("Mod+P"), "print is the browser's");
assert.ok(reserved("Mod+S"), "save is the browser's, except inside the editor where CodeMirror answers it");
assert.ok(RESERVED.length > 10, "the conflict list is a list, not a gesture");

// No chord means two things. On either platform: a binding that only collides
// on Windows is still a binding that collides.
for (const apple of [true, false]) {
  const claimed = new Map();
  for (const command of COMMANDS) {
    for (const binding of bindings(command, apple)) {
      assert.ok(!claimed.has(binding), `${binding} is claimed by both ${claimed.get(binding)} and ${command.id}`);
      claimed.set(binding, command.id);
    }
  }
}

// A platform-only binding belongs to its platform and to nowhere else.
const redo = COMMANDS.find((command) => command.id === "redo");
assert.deepEqual(bindings(redo, false), ["mod+shift+z", "mod+y"], "Ctrl+Y is offered where it is expected");
assert.deepEqual(bindings(redo, true), ["mod+shift+z"], "and ⌃Y is not offered on a Mac");

/* ----------------------------------------------------- typing beats a chord */

assert.ok(typeable("?"), "a question mark is something a person types");
assert.ok(!typeable("Mod+\\"), "⌘\\ is not");
assert.ok(!typeable("Mod+Alt+P"), "nor is ⌘⌥P");

const element = (attributes = {}, inside = null) => ({
  nodeType: 1, tagName: "DIV", isContentEditable: false,
  closest: (selector) => (inside === selector ? { nodeType: 1 } : null),
  ...attributes,
});

const editor = element({}, ".cm-editor");
const field = element({ tagName: "INPUT" });
const page = element();
const dialog = element({}, '[role="dialog"], [role="menu"], [role="alertdialog"]');

const workspace = {
  mayEdit: true, editing: true, compact: false,
  panels: ["files", "outline", "collaboration", "history", "share"],
  preview: true, compilable: true, canPreviewFile: true,
  downloads: { pdf: true, html: false, docx: false },
  edit: { undo: true, redo: true, cut: true, copy: true, paste: true, "select-all": true, find: true, replace: true },
};

const asked = (event, context = workspace) => commandFor(event, context, { apple: false })?.id ?? null;

// The rule that makes `?` usable as both a shortcut and a character: it is
// help where it could not have been text, and text everywhere else.
assert.equal(asked({ ...press("?", { shiftKey: true }), target: page }), "shortcuts");
assert.equal(asked({ ...press("?", { shiftKey: true }), target: editor }), null, "a question mark in the source is a question mark");
assert.equal(asked({ ...press("?", { shiftKey: true }), target: field }), null, "and so is one in a name field");

// A chord nobody can type keeps working from inside the editor, which is what
// ⌘\ and ⌘⌥L did before any of this existed and must go on doing.
assert.equal(asked({ ...press("\\", { ctrlKey: true }), target: editor }), "layout-cycle");
assert.equal(asked({ ...press("l", { ctrlKey: true, altKey: true, code: "KeyL" }), target: editor }), "linked");

// A dialog or a menu answers its own keys. Reaching past one would act on a
// workspace the person cannot currently see.
assert.equal(asked({ ...press("\\", { ctrlKey: true }), target: dialog }), null);
assert.equal(asked({ ...press("?", { shiftKey: true }), target: dialog }), null);

// The editor's own commands are documented here and run by CodeMirror. The
// window must not answer them too.
assert.equal(asked({ ...press("z", { ctrlKey: true }), target: page }), null, "undo is the editor's");
assert.equal(asked({ ...press("f", { ctrlKey: true }), target: page }), null, "and so is find");

// Unbound chords are left alone, so the browser keeps what it was given.
assert.equal(asked({ ...press("t", { ctrlKey: true }), target: page }), null);
assert.equal(asked({ ...press("q"), target: page }), null);

/* -------------------------------------------------- a command needs a target */

// Documented is not the same as available. A reader who was never offered the
// Files panel has no Focus Files command, and swallowing the key would be
// worse than letting the browser have it.
const reader = { ...workspace, mayEdit: false, editing: false, panels: [], preview: true, compilable: false };
assert.equal(asked({ ...press("e", { ctrlKey: true, altKey: true, code: "KeyE" }), target: page }), "focus-files");
assert.equal(asked({ ...press("e", { ctrlKey: true, altKey: true, code: "KeyE" }), target: page }, reader), null);
assert.equal(asked({ ...press("\\", { ctrlKey: true }), target: page }, reader), null, "there is no layout to cycle without a source pane");
assert.equal(asked({ ...press("?", { shiftKey: true }), target: page }, reader), "shortcuts", "help is offered to everybody");

/* ------------------------------------------------------------- the menus */

const named = menuKeys({ modalEditor: false, apple: false });
assert.equal(named.undo, "Ctrl+Z");
assert.equal(named.redo, "Ctrl+Shift+Z", "the menu names the binding the help table calls primary");
assert.equal(named.linked, "Ctrl+Alt+L");
assert.equal(named["new-file"], undefined, "a command with no binding claims no keys");

// In Vim or Emacs mode the editing keys belong to the mode. The workspace's
// own keys are not the mode's and stay named.
const modal = menuKeys({ modalEditor: true, apple: false });
assert.equal(modal.undo, undefined, "a menu naming ⌘Z in Vim mode would be lying");
assert.equal(modal.linked, "Ctrl+Alt+L");

// A greyed menu line and a dead key are the same answer to the same question,
// which is only true while both ask this.
const able = availability(workspace);
assert.equal(able.cut, true);
assert.equal(availability({ ...workspace, edit: { ...workspace.edit, cut: false } }).cut, false);
assert.equal(able["download-html"], false, "the format that is not being produced has nothing to download");
assert.equal(availability(reader)["download-pdf"], true, "and a reader may still take the PDF");
assert.equal(availability(reader).settings, false);

/* ---------------------------------------------------------- the help table */

// The consistency the spec asks a test to enforce, in both directions: every
// public shortcut has a row, and every row names a shortcut that is
// registered. Neither can drift because both are this one list.
const table = helpTable(workspace, { apple: false });
const rows = table.flatMap((group) => group.rows);
const rowIds = new Set(rows.map((row) => row.id));
for (const command of COMMANDS) {
  const bound = bindings(command, false).length > 0;
  if (bound) assert.ok(rowIds.has(command.id), `${command.id} has a shortcut and no help row`);
  if (!bound && !command.help) assert.ok(!rowIds.has(command.id), `${command.id} has a help row and no shortcut`);
}
const registered = new Set(COMMANDS.flatMap((command) => bindings(command, false).map((binding) => label(binding, false))));
for (const row of rows) {
  for (const chord of row.keys) {
    assert.ok(registered.has(chord.join("+")), `the table names ${chord.join("+")}, which nothing listens for`);
  }
}

// An alias is one row with two sets of keys, not a second row saying the same
// thing under a different name.
assert.equal(rows.filter((row) => row.id === "redo").length, 1);
assert.deepEqual(rows.find((row) => row.id === "redo").keys, [["Ctrl", "Shift", "Z"], ["Ctrl", "Y"]]);

// The table greys what this document cannot offer rather than hiding it: a key
// that disappears because a panel is closed reads as a key that does not exist.
const greyed = helpTable(reader, { apple: false }).flatMap((group) => group.rows);
assert.equal(greyed.find((row) => row.id === "focus-files").available, false);
assert.equal(greyed.find((row) => row.id === "shortcuts").available, true);
assert.equal(greyed.length, rows.length, "the same table, differently lit");

// Every category the reader will see has a name and at least one row.
for (const group of table) {
  assert.ok(group.name && group.rows.length, `an empty group: ${group.name}`);
}

/* -------------------------------------------------------------- the palette */

const list = offered(workspace, { apple: false });
assert.ok(!list.some((entry) => entry.id === "palette"), "the palette does not offer itself");
assert.ok(list.some((entry) => entry.id === "new-file"), "a command with no binding is still a command");
assert.ok(!offered(reader, { apple: false }).some((entry) => entry.id === "new-file"), "and is only offered where it can run");
assert.deepEqual(list.find((entry) => entry.id === "linked").keys, ["mod+alt+l"], "with the keys it also answers to");

// What the filter is for: a name half remembered, and a category as a way in.
assert.equal(filter(list, "download p")[0]?.id, "download-pdf");
assert.equal(filter(list, "dpdf")[0]?.id, "download-pdf", "the letters in order are enough");
assert.equal(filter(list, "down")[0]?.category, "Project");
assert.equal(filter(list, "Command palette").length, 0, "even by name, it does not offer itself");
assert.equal(filter(list, "focus f")[0]?.id, "focus-files");
assert.ok(filter(list, "workspace").every((entry) => entry.category === "Workspace"), "a category finds its commands");
assert.equal(filter(list, "")[0], list[0], "an empty query is not a filter");
assert.equal(filter(list, "zzzz").length, 0);
// What starts with the query comes before what merely contains it.
const upload = filter(list, "up");
assert.equal(upload[0].id, "upload");

console.log("commands: one notation, no browser collisions, typing beats a chord, and a help table that cannot drift from the keys");
