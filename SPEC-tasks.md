# Markdown tasks

Tasks live in Markdown task list items. The text is the only record of a task.
The visual layer draws the task's box and metadata as controls, and every
control writes the smallest change it can to the source. A project-wide Tasks
panel reads every Markdown file and lists, filters and sorts what it finds.

The request this answers is kept below the decisions, under "Original request".
Where the two disagree, the decisions win.

## Decisions

- Formats. Tasks are read in Markdown and Quarto files (`.md`, `.markdown`,
  `.qmd`). LaTeX and Typst have no tasks.
- Visual mode in LibrePaper draws over the source text; the text is the
  document. Round-trip preservation is therefore free: nothing is ever
  serialized from a model. A line the caret is on is drawn as source, as for
  every other construct, so typing on a task line shows its plain text.
- States. `[ ]` todo, `[x]` and `[X]` done, `[/]` doing, `[?]` blocked,
  `[-]` cancelled. Clicking the box toggles todo and done only. On a doing,
  blocked or cancelled task a click opens the state menu, so nothing is
  completed by accident. Right click, or the context menu key, opens the
  state menu on any task.
- Done dates. Checking a box never writes `done:`. There is no setting for
  automatic completion timestamps. A user who wants the date types it or adds
  it from the task menu. The smallest edit rule wins.
- Recurrence. Completing a task that has `repeat:` marks it `[x]` and inserts
  a new `[ ]` line after the task's block (the first line only, not nested
  content), with `due:`, `scheduled:` and `start:` advanced by one interval
  from their own values. `done:` and `cancelled:` are dropped from the copy.
  `id:` becomes a fresh unique id (`name-2`, `name-3`, ...). Month steps clamp
  to the month's last day (Jan 31 plus 1m is Feb 28 or 29). A recurring task
  with no date is completed and copied unchanged.
- Tags. `#` followed by a letter, then letters, digits, `_`, `-` and `/`.
  `#123` is not a tag. Tags are read on a task's first line only.
- Fields are read only on the first line of a task item, only when the token
  starts after a space or tab, and never inside code spans, links, URLs,
  math or escapes. An unknown `key:value` is plain text.
- Assignees are written as LibrePaper handles. They are drawn as written; the
  selector suggests every assignee already used in the project. There is no
  separate collaborator directory to resolve against.
- Dependencies. `after:` names ids anywhere in the project. In the editor a
  dependency chip opens the prerequisite. The panel warns about missing ids,
  duplicate ids and cycles.
- Validation is a warning on the chip and in the panel. Invalid text stays as
  written.
- Nested progress. A task with task children draws "n of m" after its box.
  Cancelled children are left out of both counts.
- Keyboard. The editor's controls are buttons with labels and roles; a focused
  box toggles with Space and opens its menu with Shift+F10 or the context menu
  key. Source editing reaches every field without a mouse. The Tasks panel is
  fully keyboard driven.
- Out of scope: times on dates, complex recurrence, a project setting for
  working-day length, a "today" view outside the panel.

## Code layout

- `web/src/lib/tasks/syntax.js`: states, field names, the tag and field
  grammars, validators and date arithmetic. Pure, no DOM.
- `web/src/lib/visual-markup/languages/markdown/`: the list rule reads every
  task state; a `taskmeta` inline rule emits `taskfield` and `tasktag` tokens.
- `web/src/lib/tasks/model.js`: `readTasks(source)` reads one file's tasks
  through the visual-markup tokenizer, so the editor and the panel agree.
- `web/src/lib/tasks/edit.js`: the edits, as CodeMirror change specs.
- `web/src/lib/tasks/project.js`: tasks across files, cross-file validation,
  filtering and sorting.
- `web/src/lib/visual-markup/task-widgets.js` and `tasks.css`: the controls.
- `web/src/components/reader/Tasks.svelte`: the panel.
- `web/src/lib/completion/tasks.js`: field and tag completion.

## Original request

### 1. Goals

Implement a Markdown-native task system that works consistently in Source and Visual modes.

1. All task data is stored directly in UTF-8 Markdown.
2. All syntax can be typed using an ordinary keyboard.
3. No special Unicode characters are required.
4. Standard Markdown remains compatible with other editors.
5. Custom metadata is human-readable and easy to parse.
6. Visual mode renders tasks as interactive, polished controls.
7. Switching between Source and Visual modes never loses information.
8. Editing a task changes the smallest possible portion of its source.
9. Task metadata can be searched, filtered, and aggregated across documents.
10. Tasks work without a server or separate task database.

GitHub Flavored Markdown is the baseline. Extensions degrade gracefully in
ordinary Markdown renderers.

### 2. Task states

| Syntax | State | Visual rendering |
|---|---|---|
| `[ ]` | Todo | Empty checkbox |
| `[x]` | Done | Checked checkbox |
| `[/]` | In progress | Partially filled checkbox |
| `[?]` | Blocked | Checkbox with question mark |
| `[-]` | Cancelled | Muted checkbox with dash |

Uppercase `[X]` is done and keeps its case until the state changes. Only
`[ ]` and `[x]` are GFM.

Completed tasks use muted text and strikethrough. Cancelled tasks are muted
without implying completion. Blocked tasks show a distinct indicator. An
in-progress or blocked task is never completed unless the user chooses it.

### 3. Metadata

| Field | Syntax | Meaning |
|---|---|---|
| Priority | `priority:1` | 1 critical to 5 minimal; absent means 3 |
| Due date | `due:2026-11-05` | Deadline |
| Start date | `start:2026-11-01` | Earliest actionable date |
| Scheduled date | `scheduled:2026-11-03` | Planned work date |
| Created date | `created:2026-10-30` | Date created |
| Completed date | `done:2026-11-04` | Date completed |
| Cancelled date | `cancelled:2026-11-04` | Date cancelled |
| Assignee | `assignee:alice,bob` | Assigned collaborators |
| Recurrence | `repeat:weekly` | `daily`, `weekly`, `monthly`, `yearly`, or `Nd`, `Nw`, `Nm`, `Ny` |
| Identifier | `id:task-123` | Unique task identifier |
| Dependencies | `after:task-122,review` | Prerequisite ids |
| Estimate | `estimate:2h` | `Nm`, `Nh` or `Nd` (working days) |
| Tags | `#writing` | Classification, `/` for hierarchy |

Fields may appear in any order, are separated by whitespace, are lowercase
and case-sensitive. Unknown fields stay text. Invalid values stay visible
with a warning. A repeated field keeps the source and warns. Metadata belongs
to the first line of the task; new metadata is inserted at the end of that
line. Escaped tokens (`\priority:1`) and tokens in inline code are not
metadata.

Dates are ISO calendar dates, read as local calendar days, never UTC
instants. Visual mode may show "Nov 5". Overdue incomplete tasks are
flagged; done and cancelled tasks never are.

### 4. Round trip

Switching modes changes nothing. Edits never reorder metadata, insert
defaults, normalize dates, restyle Markdown or strip unknown fields. Changing
`priority:2` to 1 replaces one character. Removing a field removes the token
and the whitespace before it.

### 5. Visual design

A task looks like a list item, not a card: the box, the text, then compact
neutral chips. Emphasis is reserved for priorities 1 and 2, blocked tasks and
overdue dates. Due date, priority and assignee are prominent; id, created,
recurrence and estimate are quieter. Every field is editable from its chip,
and the task menu adds a missing field. Status and priority are never shown
by colour alone.

### 6. Discovery

A task index is derived from the Markdown, never stored. Filter by file,
status, priority, assignee, tag, due date, overdue and dependency state. Sort
by priority, due date, created date and document order.

### 7. Non-goals

No proprietary format, task database, Unicode delimiters, per-task YAML,
hidden properties, JSON storage, external service or reformatting of
existing Markdown.
