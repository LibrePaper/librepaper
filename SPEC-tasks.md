# Markdown tasks

Markdown task items are drawn nicely in visual mode. The text is the only
record of a task, and everything but the box is edited by typing.

## Syntax

A task is a list item whose marker is followed by a box:

| Box | State |
|---|---|
| `[ ]` | To do |
| `[x]` or `[X]` | Done |
| `[/]` | In progress |
| `[-]` | Cancelled |

Only `[ ]` and `[x]` are GitHub Flavored Markdown; the others read as plain
text elsewhere. A `[?]` is not a box: the line is a plain list item.

Fields are `key:value` or `key: value` on the task's first line:

| Field | Value |
|---|---|
| `priority` | 1 (critical) to 5 (minimal) |
| `due`, `start`, `scheduled`, `created`, `done`, `cancelled` | an ISO date, `2026-11-05` |
| `assignee` | handles, comma-separated: `alice,bob` |
| `estimate` | a number with `m`, `h` or `d`: `2h` |

Tags are `#` followed by a letter, then letters, digits, `_`, `-` and `/`.

```markdown
- [ ] Write introduction priority: 1 due: 2026-11-05 #paper
- [/] Analyze survey results assignee:alice
- [x] Submit abstract done: 2026-10-30
```

## Rules

- Fields and tags are read only on a task's first line, after a space or
  tab, and never inside code, links, URLs, math or escapes. A key not in the
  table is text.
- `key:value` with no space is always a field; an invalid value is drawn as
  written with a warning. `key: value` with a space is a field only when the
  value is valid, so prose such as "what is done: the tables" stays text.
- Dates are calendar days, never UTC instants. A due date before today is
  overdue unless the task is done or cancelled.

## Visual mode

- Every box is drawn alike: one outlined square with no fill. Its state is one
  mark inside: none for to do, a check for done, a dot for in progress, a cross
  for cancelled. A click, Space or Enter moves it to the next state: `[ ]`,
  `[x]`, `[/]`, `[-]`, then `[ ]` again. `[X]` counts as `[x]`. Only the box
  character changes.
- Fields are plain text with a glyph or icon in front, not pills. Assignees
  have a user icon, or a users icon for several, then the names: "alice, bob".
  Dates have a clock icon, then the short date: "Nov 5", "start Nov 1". Priority
  is one circled digit per level: `❶` to `❺`, with 1 and 2 in the strong text
  colour, 3 in the secondary colour, and 4 and 5 faded. Overdue dates use the
  warning colour and say "overdue" in their name. Estimates are quiet text, and
  tags are small labels.
- Done tasks are muted and struck through; cancelled ones are muted.
- A parent task shows "n of m" for its task children, cancelled ones left out.
- A click on a chip puts the caret there, and the line is drawn as source
  for typing, like every other construct in visual mode.
- Drawing changes no text.

## Completion

On a task's first line the source editor completes field names, priorities,
dates (today, tomorrow, next Monday), assignees and tags already used in the
project.

## Not built

Menus, date pickers, recurrence, ids and dependencies, a task panel or index.
