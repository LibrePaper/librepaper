# SPEC: Vim keys in the editor

Status: not built. Touches `web/src/components/Editor.svelte`,
`web/src/lib/storage.js`, the layout menu in `Reader.svelte`, and the
editor rules in `web/src/styles/komodoc.css`. Nothing on the server. Reads
`04-SPEC-sync.md` for what the editor's undo means on a shared document.

## The problem

The editor is CodeMirror with the default keymap. That is the right default
and it is the only choice. A good share of the people who write typst,
Markdown or LaTeX at a terminal have Vim in their fingers, and an editor
that puts `j` on the page when they meant to move down is one they leave
open in a second tab while they edit in the first. The ask is not for a
Vim; it is for the keys.

CodeMirror 6 has a mature Vim layer, `@replit/codemirror-vim`, MIT, kept
current by Replit on the 6.x line. It does modal editing, motions, text
objects, operators, counts, registers, marks, macros, `.`, visual and
visual-block modes, Ex commands, and `Vim.map` for custom mappings. None
of that is worth writing or maintaining here. What this spec decides is
how it is switched on, where it sits among the extensions already in the
editor, and what its few seams with the rest of the page look like.

## One setting, this browser's own

```text
Keys:  ○ Default   ○ Vim
```

It is a preference of the person, not of the document: the coauthor on the
other end of the same `Y.Text` keeps whichever keys they chose. So it
lives where the pane layout lives, in `localStorage`, under
`komodoc-keys`, with the values `default` and `vim`, read through the same
`read` and `write` helpers, and falling back to `default` when storage is
off or the value is unrecognised.

It is shown in the layout menu, after the divider, beside *Keep in step*:
a menu item *Vim keys* with a check mark. That menu is described as being
"only about this arrangement", and this is not about the arrangement, but
it is the one menu that exists while editing and a menu with a single item
would be worse. It appears only while editing, because it means nothing
otherwise. There is no keyboard shortcut for the toggle, and no Ex
command for it: `:set nokeys` from inside Vim mode is a thing to find in a
menu, not a thing to guess.

The default is *Default*. Nobody is put into Vim mode who did not ask.

## Where it goes among the extensions

The package is loaded only when the setting says so, as its own chunk,
the way `Editor.svelte` is itself loaded only when someone edits: a reader
who never touches Vim never downloads it.

The extension is held in a `Compartment`, so flipping the setting
reconfigures the live editor in place. Recreating the view would drop the
caret, the scroll position and the undo history, and it would tear down
and rebuild the `yCollab` binding for a document that did not change.

Order matters. `vim()` goes first in the extension list, ahead of the
`keymap.of([...])` that carries `Mod-s`, `indentWithTab` and the default,
history, search and lint keymaps: an earlier extension has precedence, and
Vim has to see a key before the default keymap does, or `j` inserts a
letter. Keys Vim does not claim fall through to the rest, which is what
keeps `Ctrl-S`, `Ctrl-Shift-M` for the diagnostics panel, and the page's
own `Ctrl-\` and `Ctrl-Alt-L` working in both settings. `drawSelection()`
is already in the list, and it is what lets Vim draw a block cursor in
normal mode.

The mode is `vim({ status: true })`: a panel at the bottom of the editor
saying `-- INSERT --`, showing a pending count or operator, and holding the
line where `:` and `/` are typed. Without it an Ex command has nowhere to
be typed. `komodoc.css` already styles `.cm-panels` for the search and
lint panels, and the Vim panel takes the same colours, in both themes.

## Seams

**Undo.** `u` and `Ctrl-R` must do what `Ctrl-Z` and `Ctrl-Shift-Z` do
today, no more and no less, because on a shared document "undo" has a
meaning that `04-SPEC-sync.md` fixes and a second undo stack would break
it. The Vim layer routes undo through CodeMirror's commands, which is
where the default keymap's undo already goes; this is the first thing to
check when it is built, and if it is not so, the two are bound to the same
command explicitly.

**Ex commands that Vim users type without thinking.** Four are defined,
all through `Vim.defineEx`; anything else the layer already knows keeps
its meaning.

| command | does |
| --- | --- |
| `:w` | what `Mod-s` does: reports whether this browser's work has reached the server. There is no save; the status is the answer. |
| `:q` | closes the source pane, which is `cycleLayout` to the document alone |
| `:wq`, `:x` | both of the above |

`Vim` is a module-wide singleton, so an Ex command cannot close over one
component's callbacks. The handler receives the adapter, whose `cm6` is
the `EditorView`, and looks the view up in a `WeakMap` the component fills
in when it mounts; a view no longer in the map does nothing.

**Escape.** In insert mode `Esc` returns to normal mode and is consumed.
Nothing on the reader page listens for `Esc` at the window today, and
nothing may be added that would fire while the editor has focus, or the
most-typed key in Vim would start closing things.

**Registers and the clipboard.** Vim keeps its own registers. `"+y` and
`"*p` reach the system clipboard where the browser allows the page to
touch it, which is on a click or a key and over HTTPS or localhost; where
it does not, they fall back to the unnamed register, which is what the
layer does on its own.

**The other cursors.** Everyone else's caret is drawn by `yCollab` and is
not affected by the mode. `goTo`, which the preview calls to put the caret
where a click landed, dispatches a plain selection and works in normal
mode as in insert.

**Diagnostics.** `setDiagnostics`, `nextDiagnostic` and the lint panel do
not touch the keymap and are unchanged.

**Touch screens.** The setting is harmless with no keyboard, and is not
hidden; a tablet with a keyboard attached is exactly where it is wanted.

## What is not in this spec

- A way to write custom mappings, `jj` for `Esc` and the like. `Vim.map`
  makes it a few lines, but a place to type mappings and store them is a
  settings page, and this page has no settings page. When one exists,
  a mappings box goes on it.
- Keys for the preview or the comment column. Vim mode is the editor's.
- Emacs keys. The default keymap already carries the Emacs-ish subset
  CodeMirror ships, and nobody has asked.

## Steps

1. `KEYS` in `storage.js`; the menu item; the setting passed to `Editor`
   as a prop.
2. The dynamic import, the compartment, `vim()` first in the list, the
   status panel and its CSS.
3. The four Ex commands and the `WeakMap`.
4. The undo check against the sync spec, and the browser smoke in
   `web/scripts/browser-smoke.mjs`: turn Vim on, type `ihello<Esc>`, expect
   the document to read `hello`; `dd`, expect it empty; `u`, expect it
   back; `:q`, expect the source pane closed; turn Vim off, type `j`,
   expect a `j`.
