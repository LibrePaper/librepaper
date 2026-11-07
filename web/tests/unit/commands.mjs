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
  label, labelOf, menuKeys, noteOf, normalize, offered, offeredWithUnavailable, reserved, typeable,
} from "../../src/lib/commands.js";
import { MATH_TEMPLATES } from "../../src/lib/symbols/templates.js";

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

// Shift+7 is `&` on a US keyboard, and `*` for Shift+8. With Shift held, a
// digit key is read from its position, so Mod+Shift+7 is the chord on any layout
// that puts a digit on that key.
assert.equal(fromEvent(press("&", { ctrlKey: true, shiftKey: true, code: "Digit7" }), false), "mod+shift+7");
assert.equal(fromEvent(press("*", { ctrlKey: true, shiftKey: true, code: "Digit8" }), false), "mod+shift+8");
assert.equal(fromEvent(press("&", { metaKey: true, shiftKey: true, code: "Digit7" }), true), "mod+shift+7", "and on a Mac");
assert.equal(fromEvent(press("&", { ctrlKey: true, shiftKey: true, code: "" }), false), "mod+&", "without a code the key is all there is");

/* -------------------------------------------------------------- the keycaps */

assert.deepEqual(caps("Mod+Alt+P", true), ["⌘", "⌥", "P"]);
assert.deepEqual(caps("Mod+Alt+P", false), ["Ctrl", "Alt", "P"]);
assert.deepEqual(caps("Mod+\\", false), ["Ctrl", "\\"]);
assert.equal(label("Mod+Shift+Z", true), "⌘⇧Z", "a Mac menu prints the caps run together");
assert.equal(label("Mod+Shift+Z", false), "Ctrl+Shift+Z");

/* -------------------------------------------------- what the browser keeps */

// The reason this list exists: a command bound to a browser shortcut is a
// command that works until somebody presses it in a second browser. The one
// exception says so: the source text size takes the zoom keys only inside the
// source, which the editor owns, and the browser keeps them everywhere else.
for (const command of COMMANDS) {
  if (command.browserKeys) {
    assert.equal(command.owner, "editor", `${command.id} takes browser keys outside the source`);
    continue;
  }
  for (const binding of [...bindings(command, true), ...bindings(command, false)]) {
    assert.ok(!reserved(binding), `${command.id} claims ${binding}, which the browser answers`);
  }
}
assert.ok(reserved("Mod+P"), "print is the browser's");
assert.ok(!reserved("Mod+B") && !reserved("Mod+I"), "bold and italic are not the browser's");
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
  edit: {
    undo: true, redo: true, cut: true, copy: true, paste: true, "select-all": true, find: true, replace: true, bold: true, italic: true,
    link: true, comment: true, "numbered-list": true, "bulleted-list": true,
    "heading-1": true, "heading-2": true, "heading-3": true, "heading-4": true, "heading-5": true, "heading-6": true,
  },
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

// Panel-owned commands like changes keys are answered by their panel and not
// dispatched by the window.
assert.equal(asked({ ...press("a"), target: page }), null, "changes-accept is the panel's");

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
assert.equal(named.bold, "Ctrl+B");
assert.equal(named.italic, "Ctrl+I");
assert.equal(menuKeys({ modalEditor: false, apple: true }).italic, "⌘I", "a Mac menu prints the caps run together");
assert.equal(named["new-file"], undefined, "a command with no binding claims no keys");

// In Vim or Emacs mode the editing keys belong to the mode. The workspace's
// own keys are not the mode's and stay named.
const modal = menuKeys({ modalEditor: true, apple: false });
assert.equal(modal.undo, undefined, "a menu naming ⌘Z in Vim mode would be lying");
assert.equal(modal.linked, "Ctrl+Alt+L");
assert.equal(modal.bold, undefined, "Vim and Emacs keep Ctrl+B");

// A greyed menu line and a dead key are the same answer to the same question,
// which is only true while both ask this.
const able = availability(workspace);
assert.equal(able.cut, true);
assert.equal(availability({ ...workspace, edit: { ...workspace.edit, cut: false } }).cut, false);
assert.equal(able["download-html"], false, "the format that is not being produced has nothing to download");
assert.equal(availability(reader)["download-pdf"], true, "and a reader may still take the PDF");
assert.equal(availability(reader).settings, true, "a reader picks their own render engine");

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

/* ------------------------------------------------------- the palette overlays */

// The two palettes get a chord each, and the chords are the ones the browser
// does not keep. They are offered only where there is source to insert into.
assert.deepEqual(bindings(COMMANDS.find((command) => command.id === "symbols"), false), ["mod+alt+s"]);
assert.deepEqual(bindings(COMMANDS.find((command) => command.id === "math"), false), ["mod+alt+t"]);
assert.ok(["symbols", "math"].every((id) => COMMANDS.find((command) => command.id === id).category === "Insert"), "both are in the Insert category");
assert.ok(!reserved("Mod+Alt+S") && !reserved("Mod+Alt+T"), "neither chord is the browser's");

const withSource = { ...workspace, mayEdit: true, editing: true };
const withoutSource = { ...workspace, editing: false };
const offeredIds = (context) => offered(context, { apple: false }).map((entry) => entry.id);
assert.ok(offeredIds(withSource).includes("symbols"), "Insert symbol is offered with the source open");
assert.ok(offeredIds(withSource).includes("math"), "Insert math is offered with the source open");
assert.ok(offeredIds(withSource).includes("math-" + MATH_TEMPLATES[0].id), "a math template is offered with the source open");
assert.ok(!offeredIds(withoutSource).includes("symbols"), "Insert symbol is not offered without the source");
assert.ok(!offeredIds(withoutSource).includes("math"), "Insert math is not offered without the source");
assert.ok(!offeredIds(withoutSource).some((id) => id.startsWith("math-")), "and no math template is either");

// One math template command per template, named from the template, with no key.
const mathCommands = COMMANDS.filter((command) => command.category === "Math");
assert.equal(mathCommands.length, MATH_TEMPLATES.length, "one command per math template");
for (const template of MATH_TEMPLATES) {
  const command = COMMANDS.find((entry) => entry.id === "math-" + template.id);
  assert.ok(command, `math-${template.id} exists`);
  assert.equal(command.label, "Insert " + template.label.toLowerCase());
  assert.deepEqual(bindings(command, false), [], "a template has no key");
}

/* ------------------------------------------------ the bar's words, by key */

// Alt+F, E, V and H open the words of the bar, as Google Docs has them. They
// are bound on Windows and Linux only: on a Mac, Ctrl+Option is VoiceOver's.
const menuCommand = (id) => COMMANDS.find((command) => command.id === id);
for (const [letter, id] of [["f", "menu-file"], ["e", "menu-edit"], ["v", "menu-view"], ["h", "menu-help"]]) {
  assert.deepEqual(bindings(menuCommand(id), false), [`alt+${letter}`]);
  assert.deepEqual(bindings(menuCommand(id), true), [], `${id} has no Alt chord on a Mac`);
  assert.equal(menuCommand(id).palette, false, `${id} is opened by its key, not named in the palette`);
  assert.ok(!reserved(`Alt+${letter.toUpperCase()}`), `Alt+${letter.toUpperCase()} is not the browser's`);
  assert.equal(asked({ ...press(letter, { altKey: true, code: `Key${letter.toUpperCase()}` }), target: page }), id);
}

// The Alt chords are not the Mod+Alt ones, and not the editor's: Mod+Alt+F is
// replace and Mod+Alt+E is Focus Files, and both still answer to their own keys.
assert.equal(asked({ ...press("f", { ctrlKey: true, altKey: true, code: "KeyF" }), target: page }), null, "Ctrl+Alt+F on Windows is not File");
assert.equal(asked({ ...press("e", { ctrlKey: true, altKey: true, code: "KeyE" }), target: page }), "focus-files");

// Edit and View are only the reader's when the source is open, as their menus are.
assert.equal(asked({ ...press("e", { altKey: true, code: "KeyE" }), target: page }, reader), null, "no Edit menu without a source");
assert.equal(asked({ ...press("v", { altKey: true, code: "KeyV" }), target: page }, reader), null, "no View menu without a source");
assert.equal(asked({ ...press("f", { altKey: true, code: "KeyF" }), target: page }, reader), "menu-file", "File is there for everybody");
assert.equal(asked({ ...press("h", { altKey: true, code: "KeyH" }), target: page }, reader), "menu-help", "and so is Help");

// A phone has no bar, so no word of it can be opened from a key.
assert.equal(asked({ ...press("f", { altKey: true, code: "KeyF" }), target: page }, { ...workspace, compact: true }), null);

// In Vim or Emacs mode, Alt in the source belongs to the mode: Alt+F is
// forward-word in Emacs. Outside the source the chord is still the reader's.
const modalWorkspace = { ...workspace, modalEditor: true };
assert.equal(asked({ ...press("f", { altKey: true, code: "KeyF" }), target: editor }, modalWorkspace), null, "Alt+F in the source is the mode's");
assert.equal(asked({ ...press("f", { altKey: true, code: "KeyF" }), target: page }, modalWorkspace), "menu-file", "and outside the source it opens File");
assert.equal(asked({ ...press("f", { altKey: true, code: "KeyF" }), target: editor }, workspace), "menu-file", "in the default keymap Alt+F opens File from the source too");

// The help table and the palette: the chords are named where they work, and the
// palette does not offer a menu it would only open from a key.
assert.equal(menuCommand("palette").menu, "help");
assert.equal(menuCommand("shortcuts").menu, "help");
assert.ok(!offered(workspace, { apple: false }).some((entry) => entry.id.startsWith("menu-")), "the palette offers no menu chords");


/* ------------------------------------- one name, and the reason it is greyed */

// A menu prints the registry's name, so the palette and the menus cannot call
// one command two things. A name that opens a prompt says so with an ellipsis;
// one that only opens a panel or a dialog with nothing to fill in does not.
assert.equal(labelOf("linked"), "Keep in step", "the menu names it as the palette does");
assert.equal(labelOf("new-file"), "New file…", "a name that asks for a file name ends in an ellipsis");
assert.equal(labelOf("find"), "Find…", "Find asks for what to find");
assert.equal(labelOf("share"), "Share", "Share opens a panel and asks nothing");
assert.equal(labelOf("settings"), "Settings", "Settings opens a dialog and asks nothing");
assert.equal(labelOf("word-count"), "Word count", "Word count shows a count and asks nothing");
assert.equal(labelOf("no-such-command"), "", "an unknown id names nothing");

// The reason a command cannot run is the registry's note, not a menu's guess.
assert.ok(noteOf("focus-files").length > 0, "a greyed command has its reason");
assert.equal(noteOf("undo"), "", "and a command with no reason says none");

// With no query nothing is greyed, so the palette opens on what it can run.
const plain = offeredWithUnavailable(reader, "", { apple: false });
assert.deepEqual(plain.map((entry) => entry.id), offered(reader, { apple: false }).map((entry) => entry.id), "an empty query lists what is offered, and nothing else");
assert.ok(!plain.some((entry) => entry.unavailable), "and greys nothing");

// A query that matches a command this reader cannot run lists it greyed, after
// the commands that can run. Focus Preview is offered; Focus Files is not.
const asking = offeredWithUnavailable(reader, "focus", { apple: false });
const greyedAt = asking.findIndex((entry) => entry.unavailable);
assert.ok(greyedAt > 0, "the commands that can run come first");
assert.ok(asking.slice(0, greyedAt).every((entry) => !entry.unavailable), "nothing greyed sits among the available matches");
assert.ok(asking.slice(greyedAt).every((entry) => entry.unavailable), "everything after them is greyed");
assert.equal(asking.find((entry) => entry.id === "focus-preview").unavailable, false);
assert.equal(asking.find((entry) => entry.id === "focus-files").unavailable, true, "Focus Files matches but cannot run for a reader");
assert.equal(asking.find((entry) => entry.id === "focus-files").note, noteOf("focus-files"), "with its reason");
assert.ok(!asking.some((entry) => entry.id === "palette"), "the palette still does not offer itself");

// A query that matches only what cannot run lists only that, and nothing is
// selectable among it because nothing is available.
const onlyGreyed = offeredWithUnavailable(reader, "focus files", { apple: false });
assert.ok(onlyGreyed.every((entry) => entry.unavailable), "nothing a reader can run matches, so everything listed is greyed");
assert.equal(onlyGreyed[0].id, "focus-files", "and the name that starts with the query is first");

// The same name for an editor who has the panel lists it as runnable.
assert.equal(offeredWithUnavailable(workspace, "focus files", { apple: false }).find((entry) => entry.id === "focus-files").unavailable, false, "a context that can run it greys nothing");


/* ------------------------------------------------- the editing shortcuts */

const bindingsOf = (id) => bindings(COMMANDS.find((command) => command.id === id), false);

// Mod+Alt+M is the comment on the selection. It moved here from the Comments
// panel, which keeps its name and has no chord.
assert.deepEqual(bindingsOf("comment"), ["mod+alt+m"]);
assert.deepEqual(bindingsOf("focus-comments"), [], "focus comments is still in the palette, without the chord");
assert.equal(named.comment, "Ctrl+Alt+M");
assert.equal(menuKeys({ modalEditor: false, apple: true }).comment, "⌘⌥M");

// Editor-owned commands are run by CodeMirror, so the window never answers
// them, from inside the source or outside it.
const editing = { ...workspace, edit: { ...workspace.edit } };
const chord = (key, code, extra = {}) => press(key, { ctrlKey: true, altKey: true, code, ...extra });
assert.equal(asked({ ...chord("m", "KeyM"), target: editor }, editing), null, "the source answers the comment chord itself");
assert.equal(asked({ ...chord("m", "KeyM"), target: page }, editing), null, "and the window does not answer it outside the source");
assert.equal(asked({ ...press("k", { ctrlKey: true, code: "KeyK" }), target: page }, editing), null, "link is the editor's");

assert.deepEqual(bindingsOf("link"), ["mod+k"]);
assert.deepEqual(bindingsOf("numbered-list"), ["mod+shift+7"]);
assert.deepEqual(bindingsOf("bulleted-list"), ["mod+shift+8"]);
for (let level = 1; level <= 6; level += 1) {
  assert.deepEqual(bindingsOf("heading-" + level), ["mod+alt+" + level], `heading ${level} is Mod+Alt+${level}`);
}
assert.equal(named.link, "Ctrl+K");
assert.equal(named["heading-3"], "Ctrl+Alt+3");
assert.equal(named["numbered-list"], "Ctrl+Shift+7");

// The new chords are none of the browser's. Mod+U and Mod+H stay unbound.
for (const shortcut of ["Mod+K", "Mod+Alt+1", "Mod+Alt+6", "Mod+Shift+7", "Mod+Shift+8", "Mod+Alt+M"]) {
  assert.ok(!reserved(shortcut), `${shortcut} is not the browser's`);
}
for (const command of COMMANDS) {
  for (const binding of [...bindings(command, true), ...bindings(command, false)]) {
    assert.ok(binding !== "mod+u" && binding !== "mod+h", `${command.id} may not take ${binding}`);
  }
}

// The Edit menu's comment is greyed exactly when the chord would do nothing.
assert.equal(availability({ ...editing, edit: { ...editing.edit, comment: false } }).comment, false);
assert.equal(availability(editing).comment, true);

// The palette offers the markup commands where the source can take them, and
// not for a reader who has no source to write into.
const offeredOf = (context) => offered(context, { apple: false }).map((entry) => entry.id);
assert.ok(offeredOf(editing).includes("link") && offeredOf(editing).includes("heading-6") && offeredOf(editing).includes("bulleted-list"));
assert.ok(!offeredOf({ ...editing, edit: { ...editing.edit, link: false } }).includes("link"), "link is not offered where the format has none");
assert.ok(!offeredOf({ ...reader, edit: {} }).includes("heading-1"), "and not where the editor says it cannot run");

console.log("commands: one notation, no browser collisions, typing beats a chord, and a help table that cannot drift from the keys");
