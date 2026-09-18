# LibrePaper keyboard shortcuts

*2026-09-14. Implemented 2026-09-18 in `web/src/lib/commands.js` and its four
consumers: the window dispatcher in `Reader.svelte`, the menus, the command
palette, and the `?` table. Three things were decided differently while
building it, and this document is otherwise what was built.*

*The help button in the bar carries a keyboard icon rather than a `?` glyph,
because the rail below it already spends `?` on the documentation link, and it
is hidden under 600px, where the bar has no room for it and the phone has no
keyboard to explain. `Mod+Alt+F` for Replace is bound inside CodeMirror rather
than on the window, so the editor stays the only executor of its own commands.
An `Alt` chord is read from the physical key rather than the character it
produces, because `⌥P` types `π` on macOS and the whole `Mod+Alt` namespace
would otherwise be unreachable there.*

## Purpose

Give LibrePaper a predictable keyboard vocabulary for editing, navigation,
layout, preview, review, and collaboration. The vocabulary should feel close to
VS Code where that is useful, while avoiding shortcuts that browsers reserve for
tabs, navigation, reload, printing, bookmarks, downloads, or developer tools.

The `?` help entry is part of the feature. It must present the shortcuts in a
readable, platform-appropriate table and must be generated from the same command
definitions that drive keyboard handling.

## Scope

This specification covers the browser workspace, including the source editor,
preview, project panels, review panels, and menus. It does not change Vim or
Emacs keymaps inside CodeMirror. It does not define shortcuts for the landing,
sign-in, documentation, or device pages unless those pages later adopt the
shared command registry.

## Design principles

- Command names and behavior should be familiar to VS Code users.
- Browser-reserved shortcuts must remain browser shortcuts.
- Text editing shortcuts belong to the editor and must not be reimplemented by
  the workspace dispatcher.
- Global workspace shortcuts must be inactive while the user is typing in an
  input, textarea, select, contenteditable element, dialog, or menu.
- Every active shortcut must have a visible menu or help-table representation.
- Availability is contextual: a command may be documented but disabled when its
  current target does not support it.
- Mac and Windows/Linux users receive equivalent commands with native modifier
  labels.
- Shortcut handling must not depend on the document being editable unless the
  command itself changes the document.

## Shortcut notation

Implementation uses `Mod` for `Meta` on macOS and `Control` elsewhere. The help
screen renders platform-specific labels:

| Internal | macOS | Windows/Linux |
| --- | --- | --- |
| `Mod` | `⌘` | `Ctrl` |
| `Alt` | `⌥` | `Alt` |
| `Shift` | `⇧` | `Shift` |

The application namespace for global commands is `Mod+Alt`. This keeps global
commands distinct from ordinary editing commands and avoids browser shortcuts
such as `Mod+L`, `Mod+T`, `Mod+W`, `Mod+R`, and `Mod+P`.

## Initial command set

The following is the initial public shortcut contract. Labels are deliberately
phrased as commands so they can be reused in menus, the command palette, and the
help table.

| Category | Command | Shortcut | Scope | Availability |
| --- | --- | --- | --- | --- |
| Editing | Undo | `Mod+Z` | Source editor | Editor can edit |
| Editing | Redo | `Mod+Shift+Z` | Source editor | Editor can edit |
| Editing | Cut | `Mod+X` | Source editor | Editable selection |
| Editing | Copy | `Mod+C` | Source editor | Selection |
| Editing | Paste | `Mod+V` | Source editor | Editor can edit |
| Editing | Select All | `Mod+A` | Source editor | Editor focused |
| Editing | Find | `Mod+F` | Source editor | Editor mounted |
| Editing | Replace | `Mod+Alt+F` | Source editor | Editor can edit |
| Navigation | Command palette | `Mod+Alt+P` | Workspace | Workspace active |
| Navigation | Focus Files | `Mod+Alt+E` | Workspace | Files panel available |
| Navigation | Focus Outline | `Mod+Alt+O` | Workspace | Outline available |
| Navigation | Focus Comments | `Mod+Alt+M` | Workspace | Comments available |
| Navigation | Focus Preview | `Mod+Alt+V` | Workspace | Preview visible |
| Workspace | Toggle source / preview / split | `Mod+\\` | Workspace | Workspace active |
| Workspace | Toggle linked scrolling | `Mod+Alt+L` | Workspace | Source editor active |
| Workspace | Source on left/right | command palette only | Workspace | Wide layout |
| Preview | Compile now | `Mod+Alt+B` | Workspace | Compilable editable document |
| Project | Open Settings | `Mod+,` | Workspace | Workspace active |
| Help | Show keyboard shortcuts | `?` | Workspace | No text field is active |
| Help | Show keyboard shortcuts | `Mod+Alt+/` | Workspace | Workspace active |

`Mod+Alt+P` is intentionally assigned to the command palette; the preview is
available through the command palette and its existing controls. If a dedicated
preview shortcut proves necessary, it should use a later unassigned command
rather than reclaiming a browser-reserved `Mod+P`.

The existing `Mod+\\` layout toggle and `Mod+Alt+L` linked-scrolling shortcut
remain compatible with this contract. `Mod+S` continues to report persistence
status in the editor/workspace; it must not claim that unsent changes have been
saved.

## Browser-conflict policy

The application must not register global commands for these families:

- `Mod+L`, `Mod+T`, `Mod+W`, `Mod+N`, `Mod+R`, `Mod+Shift+T`
- `Mod+D`, `Mod+J`, `Mod+H`, `Mod+U`, `Mod+P`, `Mod+S`
- `F5`, `F6`, `F11`, `F12`, and browser zoom shortcuts
- `Alt+Left`, `Alt+Right`, and other browser history/navigation commands

Some browsers reserve additional combinations by platform. The implementation
must maintain a small conflict list and test on the supported browser matrix.
When a browser wins a race and the event cannot be cancelled, LibrePaper must
not advertise that shortcut as an application command.

`Mod+S` is the only exception to the global list in the initial contract: it is
handled by CodeMirror while the source editor is focused, where it prevents the
browser save-page action and reports the document's actual persistence state.

## Command registry

Add a shared browser-side command registry, preferably
`web/src/lib/commands.js`. Each command definition contains:

- stable `id`;
- display `label` and `category`;
- normalized shortcut descriptor;
- scope (`editor`, `workspace`, `preview`, or `review`);
- availability predicate;
- action callback;
- optional menu location and help description.

The registry is descriptive and executable: the same definitions produce the
keyboard dispatcher, menu shortcut labels, command palette entries, and the `?`
table. No shortcut should be duplicated as an unrelated literal in a component.

The registry must support aliases where platform or editor conventions require
them, but aliases must still be listed once in the help table. It must also
support commands with no shortcut, such as layout ratios or source-side choice.

## Event ownership and focus rules

Use one workspace-level dispatcher for commands outside CodeMirror. It should:

1. normalize `Meta`/`Control`, `Alt`, `Shift`, key names, and punctuation;
2. identify the active command and check its availability;
3. ignore text-entry controls, contenteditable elements, dialogs, and open
   menus;
4. call `preventDefault()` only after a matching available command is found;
5. invoke the command and return focus to the appropriate target when needed.

CodeMirror remains responsible for editor-local commands, including native
editing, search, completion, lint navigation, Vim, and Emacs behavior. The
workspace dispatcher must not compete with those keymaps. The editor may expose
its commands to the registry for documentation and menu state without moving
their implementation out of CodeMirror.

The `?` key is special. It opens help only when focus is not in a text-entry
context. Typing `?` in the source editor, chat composer, comments, settings, or
an ordinary form field must insert or type the character normally.

## Help menu and shortcut table

Add a `?` icon button to the workspace navigation bar. It opens a modal titled
`Keyboard shortcuts`. The button must have an accessible name and tooltip, and
the modal must be reachable with the keyboard.

The modal contains a visually polished table with:

- grouped category headings;
- command name;
- rendered shortcut keys as compact keycaps;
- context or availability note where the command is not global;
- muted rows for unavailable contextual commands only when showing the full
  command set is useful;
- a short note that browser shortcuts and editor mode keymaps are not replaced.

The table must remain usable on narrow screens. On compact layouts it may become
stacked cards, but it must preserve the command, shortcut, and context
relationship. It must support light and dark themes and meet the existing
keyboard-focus and contrast conventions for `Modal.svelte`.

The table is generated from the command registry. A unit test must fail if a
public command has a shortcut but no help-table representation, or if the help
table names a shortcut that is not registered.

## Menus and command palette

Existing File, Edit, View, and Tools menus should show shortcut labels for
commands they expose. Menu actions and keyboard actions must call the same
command callback. Disabled menu items and unavailable shortcuts must use the
same availability predicate.

Add a command palette using the registry. It should filter by command label and
category, show the shortcut beside each result, skip unavailable commands, and
execute the selected command without requiring a second implementation path.

The command palette itself is not a browser search field. It must close on
`Escape`, restore focus to its opener, and avoid capturing typing once closed.

## Vim and Emacs modes

The existing per-editor Vim and Emacs modes remain authoritative for their
mode-specific keymaps. The help table should include a short contextual note:

> Vim and Emacs mode shortcuts are provided by the selected editor mode. The
> workspace shortcuts above remain available where they do not conflict with
> that mode.

Do not promise that every CodeMirror or Vim/Emacs command appears in the main
LibrePaper table. If mode-specific documentation is added later, it should be a
separate section generated by the editor-mode integration.

## Implementation sequence

1. Extract the current `Reader.svelte` global shortcuts into command definitions
   without changing their behavior.
2. Add normalized shortcut matching and focus/context guards.
3. Register existing menu commands and editor commands, keeping CodeMirror as
   the executor for editor-local behavior.
4. Add command-palette support and shortcut labels to existing menus.
5. Add the `?` button and responsive keyboard-shortcut modal.
6. Add unit, browser, and accessibility coverage for conflicts, focus behavior,
   platform rendering, and registry/help-table consistency.
7. Remove duplicated shortcut literals after all consumers use the registry.

## Acceptance criteria

- Existing `Mod+\\` and `Mod+Alt+L` behavior remains intact.
- Undo, redo, clipboard, search, completion, Vim, and Emacs behavior remains
  owned by CodeMirror and passes existing editor tests.
- Global shortcuts do not fire while typing in editor, chat, comments, forms,
  dialogs, or menus, except for intentionally editor-owned shortcuts.
- Browser navigation, tab, reload, print, bookmark, download, history, zoom,
  and developer-tool shortcuts remain browser-owned.
- Every registered public shortcut appears in the `?` table with a correct
  platform-specific rendering.
- Every menu shortcut invokes the same command action as its keyboard shortcut.
- The help modal is keyboard accessible, responsive, theme-aware, and restores
  focus when closed.
- The command palette filters available commands, displays shortcuts, executes
  the selected action, and closes cleanly.
- Mac and Windows/Linux modifier labels are correct.
- Shortcut unit and browser tests cover punctuation keys, modifier normalization,
  text-entry suppression, browser-conflict exclusions, and unavailable commands.
- No shortcut is advertised unless its command is implemented and reachable in
  the current application state.

## Non-goals

- Replacing CodeMirror's default, Vim, or Emacs keymaps.
- Preventing users from using browser shortcuts.
- Making every VS Code shortcut identical in a browser environment.
- Adding a user-configurable keybinding editor in the first implementation.
- Assigning shortcuts to every menu item.
- Changing server, collaboration, persistence, or document-edit semantics.
