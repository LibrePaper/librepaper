// What the workspace can be asked to do, written once.
//
// A shortcut used to be a literal in whichever component happened to hear the
// key, and the name of that shortcut a second literal in whichever menu
// happened to offer it. Two copies of one fact: the menu could name a key the
// window no longer listened for, and nothing would say so. This list is the
// fact. The keyboard reads it to decide what a chord means, the menus read it
// to print the keys beside a name, the palette reads it to offer the name, and
// the help table is nothing but this list drawn.
//
// The module is pure on purpose. It knows the shape of the workspace -- what a
// panel is, whether there is a preview -- but touches no state and no DOM, so
// every rule below is reachable from a plain Node test.

/// Whether the keyboard in front of us writes ⌘ or Ctrl. Read once at import,
/// from whatever `navigator` this runtime has, because it cannot change while
/// the page is open.
export const APPLE = applePlatform(globalThis.navigator);

export function applePlatform(nav) {
  if (!nav) return false;
  return /Mac|iPhone|iPad|iPod/.test(nav.platform || nav.userAgent || "");
}

/* ------------------------------------------------------------- the notation */

// `Mod` is ⌘ on a Mac and Ctrl everywhere else, which is the only difference
// between the two platforms this file admits. Everything else is spelled the
// same and drawn differently.
const MODIFIERS = ["mod", "alt", "shift"];

/// A written shortcut (`"Mod+Alt+P"`, `"Mod+\\"`, `"?"`) as a canonical string
/// (`"mod+alt+p"`, `"mod+\\"`, `"?"`). The canonical form is what the
/// dispatcher compares against, so it is also what a duplicate binding would
/// collide on.
///
/// Shift is dropped from a chord whose key is punctuation: `?` is typed with
/// Shift on most layouts and not on all of them, and a binding that insisted
/// on the modifier would be a binding that worked on one keyboard.
export function normalize(descriptor) {
  const parts = String(descriptor).split("+").map((part) => part.trim()).filter(Boolean);
  // "Mod++" and "Mod+\\": the key is the last part, unless splitting ate it,
  // in which case the descriptor ended with the separator itself.
  const key = String(descriptor).endsWith("+") ? "+" : parts.pop() || "";
  const held = new Set(parts.map((part) => part.toLowerCase()));
  const named = key.toLowerCase();
  if (!letters(named)) held.delete("shift");
  return [...MODIFIERS.filter((name) => held.has(name)), named].join("+");
}

const letters = (key) => /^[a-z0-9]$/.test(key) || key.length > 1;

/// The canonical form of a key event, or "" when the event is a modifier being
/// pressed on its own or a chord this application never claims.
///
/// The modifier that is not `Mod` on this platform disqualifies the event
/// rather than being ignored: Ctrl+Alt+L on a Mac is not ⌘⌥L, and a dispatcher
/// that treated it as one would swallow a chord the window manager sent.
export function fromEvent(event, apple = APPLE) {
  const key = event.key;
  if (!key || ["Control", "Meta", "Alt", "Shift"].includes(key)) return "";
  if (apple ? event.ctrlKey : event.metaKey) return "";
  const held = [];
  if (apple ? event.metaKey : event.ctrlKey) held.push("mod");
  if (event.altKey) held.push("alt");
  const named = (event.altKey ? typed(event.code) || key : key).toLowerCase();
  if (event.shiftKey && letters(named)) held.push("shift");
  return [...MODIFIERS.filter((name) => held.includes(name)), named].join("+");
}

// Which key was pressed, when Alt says the character cannot answer that.
//
// Alt is a compose key on macOS: ⌥P is "π", ⌥/ is "÷", and a chord read from
// `event.key` alone would be a chord no Mac can press. The physical key is
// asked instead. This makes an Alt chord follow the key's position rather than
// its letter, which is the trade every editor with an Alt namespace makes: the
// alternative is a namespace that works on one keyboard.
const PUNCTUATION = {
  Slash: "/", Backslash: "\\", Comma: ",", Period: ".", Semicolon: ";", Quote: "'",
  BracketLeft: "[", BracketRight: "]", Minus: "-", Equal: "=", Backquote: "`",
};

function typed(code) {
  if (!code) return "";
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  return PUNCTUATION[code] || "";
}

const CAPS = {
  apple: { mod: "⌘", alt: "⌥", shift: "⇧" },
  other: { mod: "Ctrl", alt: "Alt", shift: "Shift" },
};

const KEY_CAPS = { escape: "Esc", enter: "Enter", " ": "Space", arrowleft: "←", arrowright: "→", arrowup: "↑", arrowdown: "↓" };

/// A shortcut as the keycaps to print, in the order a person reads them.
/// Returned as an array rather than a string because the help table and the
/// palette draw each cap in its own box, while a menu joins them.
export function caps(descriptor, apple = APPLE) {
  const canonical = normalize(descriptor);
  const parts = canonical.split("+");
  const key = canonical.endsWith("++") || canonical === "+" ? "+" : parts.pop();
  const names = CAPS[apple ? "apple" : "other"];
  const printed = MODIFIERS.filter((name) => parts.includes(name)).map((name) => names[name]);
  printed.push(KEY_CAPS[key] || (key.length === 1 ? key.toUpperCase() : key));
  return printed;
}

/// The same, as one string, for a menu line where a row of boxes would be
/// furniture. A Mac joins the caps with nothing, which is how a Mac menu
/// prints them; everywhere else they are joined with a plus.
export const label = (descriptor, apple = APPLE) => caps(descriptor, apple).join(apple ? "" : "+");

/* -------------------------------------------------------- what browsers own */

// Shortcuts the browser answers before the page hears them, or answers anyway
// when the page tries to. A command registered on one of these is a command
// that works until somebody presses it in a second browser, so the registry
// refuses them outright and a unit test walks the list.
//
// `Mod+S` is the one exception and it is not registered here: CodeMirror binds
// it inside the source editor, where it reports what has actually been saved
// rather than claiming a save the browser would have done to a file.
export const RESERVED = [
  "mod+l", "mod+t", "mod+w", "mod+n", "mod+r", "mod+shift+t",
  "mod+d", "mod+j", "mod+h", "mod+u", "mod+p", "mod+s",
  "f5", "f6", "f11", "f12",
  "mod++", "mod+-", "mod+0",
  "alt+arrowleft", "alt+arrowright",
];

export const reserved = (descriptor) => RESERVED.includes(normalize(descriptor));

/* ------------------------------------------------------------ what is typing */

/// Whether an element is somewhere a person is writing. A global chord that
/// fired while somebody was naming a file would rename the file and open a
/// panel, so anything holding text answers yes.
export function writing(element) {
  if (!element || element.nodeType !== 1) return false;
  const tag = (element.tagName || "").toLowerCase();
  if (["input", "textarea", "select"].includes(tag)) return true;
  if (element.isContentEditable) return true;
  return Boolean(element.closest?.(".cm-editor"));
}

/// Whether an element is inside something that has taken the page over: a
/// dialog traps the focus and a menu is walked with the keyboard, and neither
/// is a place where a workspace shortcut should reach past what is open.
export function captured(element) {
  if (!element || element.nodeType !== 1) return false;
  return Boolean(element.closest?.('[role="dialog"], [role="menu"], [role="alertdialog"]'));
}

/// Whether a chord can be typed as text. `?` can, `⌘⌥P` cannot, and that is
/// the whole of the rule that keeps `?` from opening help while somebody is
/// writing a question and still lets ⌘\ switch the layout from inside the
/// editor.
export const typeable = (descriptor) => {
  const canonical = normalize(descriptor);
  return !canonical.startsWith("mod+") && !canonical.startsWith("alt+");
};

/* -------------------------------------------------------------- the commands */

// A definition is: what it is called, where it belongs, who executes it, what
// it is bound to, and when it can be asked for.
//
// `owner` is "editor" for the commands CodeMirror runs. Those are listed here
// so the menus can print their keys and the help table can name them, and the
// window dispatcher never registers them: undo inside a text editor belongs to
// the text editor, and a second implementation on the window would be a second
// answer to what undo means.
//
// `keys` is the bindings, most-preferred first. A platform-only binding is
// written as an object with `apple` or `other`, which is how redo carries
// Ctrl+Y on the keyboards that expect it without offering ⌃Y on a Mac.
//
// `available` is asked about a context the reader assembles: this file decides
// the rule, the reader answers the facts. A command with no `available` is
// always offered.

const ALWAYS = () => true;
const panel = (id) => (context) => (context.panels || []).includes(id);

export const COMMANDS = [
  // Editing. CodeMirror's, every one of them, including the search panel.
  { id: "undo", label: "Undo", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+Z"], available: (c) => Boolean(c.edit?.undo) },
  { id: "redo", label: "Redo", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+Shift+Z", { other: "Mod+Y" }], available: (c) => Boolean(c.edit?.redo) },
  { id: "cut", label: "Cut", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+X"], available: (c) => Boolean(c.edit?.cut) },
  { id: "copy", label: "Copy", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+C"], available: (c) => Boolean(c.edit?.copy) },
  { id: "paste", label: "Paste", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+V"], available: (c) => Boolean(c.edit?.paste) },
  { id: "select-all", label: "Select All", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+A"], available: (c) => Boolean(c.edit?.["select-all"]) },
  { id: "find", label: "Find", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+F"], available: (c) => Boolean(c.edit?.find) },
  { id: "replace", label: "Replace", category: "Editing", scope: "editor", owner: "editor", menu: "edit", keys: ["Mod+Alt+F"], available: (c) => Boolean(c.edit?.replace) },

  // Navigation. Where the focus goes, which is the part of a workspace a
  // pointer does well and a keyboard has to be told about.
  { id: "palette", label: "Command palette", category: "Navigation", scope: "workspace", keys: ["Mod+Alt+P"], palette: false, available: ALWAYS },
  { id: "focus-files", label: "Focus Files", category: "Navigation", scope: "workspace", keys: ["Mod+Alt+E"], available: panel("files"), note: "Files panel" },
  { id: "focus-outline", label: "Focus Outline", category: "Navigation", scope: "workspace", keys: ["Mod+Alt+O"], available: panel("outline"), note: "Outline panel" },
  { id: "focus-comments", label: "Focus Comments", category: "Navigation", scope: "workspace", keys: ["Mod+Alt+M"], available: panel("collaboration"), note: "Comments panel" },
  { id: "focus-preview", label: "Focus Preview", category: "Navigation", scope: "workspace", keys: ["Mod+Alt+V"], available: (c) => Boolean(c.preview), note: "Preview visible" },

  // The workspace itself: which panes are up, and whether they scroll
  // together. The two that had shortcuts before this file existed keep them.
  { id: "layout-cycle", label: "Toggle source / preview / split", category: "Workspace", scope: "workspace", keys: ["Mod+\\"], available: (c) => Boolean(c.editing) },
  { id: "linked", label: "Toggle linked scrolling", category: "Workspace", scope: "workspace", menu: "view", keys: ["Mod+Alt+L"], available: (c) => Boolean(c.editing) },
  { id: "layout-source", label: "Show source only", category: "Workspace", scope: "workspace", keys: [], available: (c) => Boolean(c.editing) },
  { id: "layout-document", label: "Show preview only", category: "Workspace", scope: "workspace", keys: [], available: (c) => Boolean(c.editing) },
  { id: "layout-split", label: "Show source and preview", category: "Workspace", scope: "workspace", keys: [], available: (c) => Boolean(c.editing) && !c.compact },
  { id: "side-left", label: "Source on left", category: "Workspace", scope: "workspace", help: true, keys: [], available: (c) => Boolean(c.editing) && !c.compact, note: "Wide layout" },
  { id: "side-right", label: "Source on right", category: "Workspace", scope: "workspace", help: true, keys: [], available: (c) => Boolean(c.editing) && !c.compact, note: "Wide layout" },

  // The preview.
  { id: "compile", label: "Compile now", category: "Preview", scope: "preview", menu: "file", keys: ["Mod+Alt+B"], available: (c) => Boolean(c.compilable), note: "Compiled document" },
  { id: "preview-file", label: "Preview this file", category: "Preview", scope: "preview", menu: "view", keys: [], available: (c) => Boolean(c.canPreviewFile) },

  // The project. None of these is bound to anything: they are here so the
  // palette can offer them by name and so the one list stays the one list.
  { id: "new-file", label: "New file", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.mayEdit) },
  { id: "new-folder", label: "New folder", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.mayEdit) },
  { id: "upload", label: "Upload files", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.mayEdit) },
  { id: "download-pdf", label: "Download PDF", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.downloads?.pdf) },
  { id: "download-html", label: "Download HTML", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.downloads?.html) },
  { id: "download-docx", label: "Download DOCX", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.downloads?.docx) },
  { id: "download", label: "Download project", category: "Project", scope: "workspace", menu: "file", keys: [], available: (c) => Boolean(c.mayEdit) },
  { id: "share", label: "Share", category: "Project", scope: "workspace", menu: "file", keys: [], available: panel("share") },
  { id: "history", label: "History", category: "Project", scope: "workspace", menu: "file", keys: [], available: panel("history") },
  { id: "settings", label: "Open Settings", category: "Project", scope: "workspace", menu: "file", keys: ["Mod+,"], available: (c) => Boolean(c.mayEdit) },

  // Help, which is this list about itself.
  { id: "shortcuts", label: "Show keyboard shortcuts", category: "Help", scope: "workspace", keys: ["?", "Mod+Alt+/"], available: ALWAYS, note: "Not while typing" },
];

/// The bindings a command has on this platform, canonical, most-preferred
/// first. A `{apple}`/`{other}` entry that belongs to the other platform is
/// dropped rather than rendered, which is why the help table never prints a
/// key the keyboard in front of it does not have.
export function bindings(command, apple = APPLE) {
  return (command.keys || [])
    .map((entry) => (typeof entry === "string" ? entry : apple ? entry.apple : entry.other))
    .filter(Boolean)
    .map((descriptor) => normalize(descriptor));
}

/// Every command, in the order the help table prints them, grouped.
export function categories(commands = COMMANDS) {
  const order = [];
  const groups = new Map();
  for (const command of commands) {
    if (!groups.has(command.category)) { groups.set(command.category, []); order.push(command.category); }
    groups.get(command.category).push(command);
  }
  return order.map((name) => ({ name, commands: groups.get(name) }));
}

/// Which command a chord asks for, or null. `commands` is passed in so a test
/// can ask the question of a list it wrote itself.
///
/// The editor's own commands are not matched here. They are in the list for
/// the menus and the help table; the window must not run a second undo.
export function match(canonical, { commands = COMMANDS, apple = APPLE } = {}) {
  if (!canonical) return null;
  for (const command of commands) {
    if (command.owner === "editor") continue;
    if (bindings(command, apple).includes(canonical)) return command;
  }
  return null;
}

/// What a key event should do, given where it landed and what the workspace
/// can currently offer. Returns the command to run, or null.
///
/// The three refusals, in the order they are asked:
///
/// 1. A dialog or a menu is open under the event. Whatever is open answers its
///    own keys, including Escape, and a workspace shortcut reaching past it
///    would act on a workspace the person cannot see.
/// 2. The chord is one a person could type and they are typing. `?` in the
///    source, the chat composer, a comment or a file-name field is a question
///    mark; it is only help when it could not have been text.
/// 3. The command exists but its target does not. A documented command whose
///    panel this reader was never offered is not a command, and swallowing the
///    key would be worse than letting the browser have it.
export function commandFor(event, context = {}, { commands = COMMANDS, apple = APPLE } = {}) {
  const target = event.target;
  if (captured(target)) return null;
  const canonical = fromEvent(event, apple);
  const command = match(canonical, { commands, apple });
  if (!command) return null;
  if (typeable(canonical) && writing(target)) return null;
  const available = command.available || ALWAYS;
  return available(context) ? command : null;
}

/// The keys to print beside a name in a menu, by command id. Empty for a
/// command with no binding, and empty for every editing command while a modal
/// editor mode is on: in Vim or Emacs mode those keys belong to the mode, and
/// a menu naming them would be lying about what the next keystroke does.
export function menuKeys({ modalEditor = false, apple = APPLE, commands = COMMANDS } = {}) {
  const printed = {};
  for (const command of commands) {
    if (modalEditor && command.scope === "editor") continue;
    const [first] = bindings(command, apple);
    if (first) printed[command.id] = label(first, apple);
  }
  return printed;
}

/// What the palette offers: everything that can be asked for by name right
/// now, with the keys it also answers to. The palette itself is left out --
/// offering "Command palette" inside the command palette is a line that can
/// only close what opened it.
export function offered(context = {}, { commands = COMMANDS, apple = APPLE } = {}) {
  return commands
    .filter((command) => command.palette !== false)
    .filter((command) => (command.available || ALWAYS)(context))
    .map((command) => ({ ...command, keys: bindings(command, apple) }));
}

/// The palette's filter. Matching the category as well as the label is what
/// makes "view" find the layout commands, and the subsequence match is what
/// makes "dpdf" find Download PDF. Ordered: what starts with the query first,
/// then what contains it, then the rest.
export function filter(entries, query) {
  const needle = query.trim().toLowerCase();
  if (!needle) return entries;
  const scored = [];
  for (const entry of entries) {
    const name = entry.label.toLowerCase();
    const haystack = `${name} ${entry.category.toLowerCase()}`;
    if (name.startsWith(needle)) scored.push([0, entry]);
    else if (haystack.includes(needle)) scored.push([1, entry]);
    else if (subsequence(haystack, needle)) scored.push([2, entry]);
  }
  return scored.sort((a, b) => a[0] - b[0]).map(([, entry]) => entry);
}

function subsequence(haystack, needle) {
  let at = 0;
  for (const character of needle) {
    at = haystack.indexOf(character, at);
    if (at === -1) return false;
    at += 1;
  }
  return true;
}

/// The help table, generated rather than written: a command is a row when it
/// has a binding on this platform, or when it asked to be named without one
/// because the palette is the only way to reach it.
///
/// Each row carries the caps for every binding it answers to, so an alias is
/// one row with two sets of keys rather than a second line saying the same
/// thing, and whether it is available right now, so the table can grey what
/// this document cannot currently offer instead of hiding it.
export function helpTable(context = {}, { commands = COMMANDS, apple = APPLE } = {}) {
  const shown = commands.filter((command) => command.help || bindings(command, apple).length);
  return categories(shown).map((group) => ({
    name: group.name,
    rows: group.commands.map((command) => ({
      id: command.id,
      label: command.label,
      keys: bindings(command, apple).map((binding) => caps(binding, apple)),
      note: command.note || "",
      available: (command.available || (() => true))(context),
    })),
  }));
}

/// Every command's availability at once, by id. The menus ask this rather than
/// each item deciding for itself, so a greyed line and a dead key are the same
/// answer to the same question: a menu that greys Download PDF while the
/// shortcut still fires is two rules disagreeing in public.
export function availability(context = {}, { commands = COMMANDS } = {}) {
  const answers = {};
  for (const command of commands) answers[command.id] = (command.available || ALWAYS)(context);
  return answers;
}
