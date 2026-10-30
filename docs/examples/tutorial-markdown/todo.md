# To do

Turn on the Visual mode switch in the top bar to see this list drawn as a
tidy to-do list, with checkboxes, priorities, dates and people.

Tasks are ordinary Markdown list items with a box. In visual mode, click a
box to move it along: to do, done, in progress, cancelled, and back.

## This week

- [ ] Draft the introduction priority: 1 due: 2026-11-06 assignee: alice #paper
- [/] Clean the survey data priority: 2 assignee: bob estimate: 3h #analysis
- [ ] Run the robustness checks start: 2026-11-04 due: 2026-11-10 assignee: alice,bob #analysis
- [x] Send the abstract to the workshop done: 2026-10-28 #paper
- [-] Book a room for the pilot cancelled: 2026-10-27

## Submission checklist

- [ ] Prepare the manuscript due: 2026-11-20 priority: 2
  - [x] Abstract
  - [ ] Main text
  - [ ] Figures
  - [ ] Supplementary materials

## Writing tasks by hand

Fields go on the task's first line, as `key: value` or `key:value`:

- `priority: 1` (critical) to `priority: 5` (minimal)
- `due:`, `start:`, `scheduled:`, `created:`, `done:` and `cancelled:` take a date such as `2026-11-05`
- `assignee: alice,bob` names one person or several
- `estimate: 2h` takes minutes, hours or days: `30m`, `2h`, `1d`
- `#tags` group tasks

Every box writes plain Markdown, so the file reads the same in any editor.
