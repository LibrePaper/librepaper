<script>
  // The project's Tasks panel. It filters, sorts and draws the tasks Reader
  // reads from every Markdown file, and reports what was chosen. It never
  // writes a file itself: a change goes to Reader, which makes it in the
  // document the editor is showing.
  import Icon from "../Icon.svelte";
  import { PRIORITIES, STATES, isOverdue, shortDate, todayISO } from "../../lib/tasks/syntax.js";
  import { filterTasks, sortTasks, taskVocabulary } from "../../lib/tasks/project.js";

  let { tasks = [], today = todayISO(), onselect, onchange, tag = "" } = $props();

  const STATUS = [
    { value: "open", label: "Open" },
    { value: "all", label: "All" },
    ...STATES.map(({ state, label }) => ({ value: state, label })),
  ];
  const DUE = [
    { value: "", label: "Any" },
    { value: "overdue", label: "Overdue" },
    { value: "today", label: "Today" },
    { value: "week", label: "Next 7 days" },
    { value: "none", label: "No date" },
  ];
  const SORTS = [
    { value: "document", label: "Document" },
    { value: "priority", label: "Priority" },
    { value: "due", label: "Due date" },
    { value: "created", label: "Created" },
  ];
  const STATE_LABEL = Object.fromEntries(STATES.map(({ state, label }) => [state, label]));
  const PRIORITY_LABEL = Object.fromEntries(PRIORITIES.map(({ value, label }) => [value, label]));

  let query = $state("");
  let status = $state("open");
  /** @type {"" | "overdue" | "today" | "week" | "none"} */
  let due = $state("");
  let priority = $state("");
  let assignee = $state("");
  let tagFilter = $state("");
  let path = $state("");
  let sort = $state("document");
  let waiting = $state(false);

  // A tag chosen elsewhere in the editor opens the panel filtered to it. Only a
  // change of the prop sets the filter, so the reader can still change it.
  $effect(() => {
    tagFilter = tag;
  });

  const vocabulary = $derived(taskVocabulary(tasks));
  // A value the project does not hold, such as a tag passed in, stays in its
  // select so that the select says what is being filtered.
  const withCurrent = (names, current) => (current && !names.includes(current) ? [...names, current].sort() : names);
  const assignees = $derived(withCurrent(vocabulary.assignees, assignee));
  const tags = $derived(withCurrent(vocabulary.tags, tagFilter));
  const files = $derived([...new Set(tasks.map((task) => task.path))]);

  const filter = $derived({
    states: status === "all" ? undefined : status === "open" ? ["todo", "doing", "blocked"] : [status],
    due: due || undefined,
    priority: priority ? Number(priority) : undefined,
    assignee: assignee || undefined,
    tag: tagFilter || undefined,
    path: path || undefined,
    blocked: waiting || undefined,
    query: query.trim() || undefined,
  });
  const shown = $derived(sortTasks(filterTasks(tasks, filter, today), sort));
  const countText = $derived(shown.length === 1 ? "1 task" : `${shown.length.toLocaleString()} tasks`);

  // Under the document order a file's tasks are next to each other, so each
  // file gets one heading. Any other order mixes the files, and each row names
  // its file instead.
  const groups = $derived.by(() => {
    if (sort !== "document") return [{ path: "", tasks: shown }];
    const out = [];
    for (const task of shown) {
      const last = out.at(-1);
      if (last && last.path === task.path) last.tasks.push(task);
      else out.push({ path: task.path, tasks: [task] });
    }
    return out;
  });

  // The box shows a task's state. A doing task has no checked state, so it is
  // drawn indeterminate, and only the property can say that. A click clears
  // indeterminate in the browser, so the handler draws the state again.
  function showState(node, task) {
    node.checked = task.state === "done";
    node.indeterminate = task.state === "doing";
  }
  function box(node, task) {
    showState(node, task);
    return { update: (next) => showState(node, next) };
  }

  // The assignees a task names, in the order they are written.
  function assigneesOf(task) {
    return task.fields
      .filter((field) => field.key === "assignee" && field.value)
      .flatMap((field) => field.value.split(","))
      .filter(Boolean);
  }
</script>

<section class="tasks" aria-label="Project tasks">
  <h2 class="sr-only">Project tasks</h2>
  {#if !tasks.length}
    <p class="tasks-empty">No tasks in this project.</p>
  {:else}
    <div class="tasks-filters" role="search">
      <label class="tasks-field tasks-search">
        <span class="tasks-label">Search</span>
        <input type="search" bind:value={query} placeholder="Search tasks" />
      </label>
      <label class="tasks-field">
        <span class="tasks-label">Status</span>
        <select bind:value={status}>
          {#each STATUS as option (option.value)}<option value={option.value}>{option.label}</option>{/each}
        </select>
      </label>
      <label class="tasks-field">
        <span class="tasks-label">Due</span>
        <select bind:value={due}>
          {#each DUE as option (option.value)}<option value={option.value}>{option.label}</option>{/each}
        </select>
      </label>
      <label class="tasks-field">
        <span class="tasks-label">Priority</span>
        <select bind:value={priority}>
          <option value="">Any</option>
          {#each PRIORITIES as option (option.value)}<option value={String(option.value)}>{option.value} {option.label}</option>{/each}
        </select>
      </label>
      <label class="tasks-field">
        <span class="tasks-label">Assignee</span>
        <select bind:value={assignee}>
          <option value="">Anyone</option>
          {#each assignees as name (name)}<option value={name}>{name}</option>{/each}
        </select>
      </label>
      <label class="tasks-field">
        <span class="tasks-label">Tag</span>
        <select bind:value={tagFilter}>
          <option value="">Any tag</option>
          {#each tags as name (name)}<option value={name}>#{name}</option>{/each}
        </select>
      </label>
      <label class="tasks-field">
        <span class="tasks-label">File</span>
        <select bind:value={path}>
          <option value="">All files</option>
          {#each files as name (name)}<option value={name}>{name}</option>{/each}
        </select>
      </label>
      <label class="tasks-field">
        <span class="tasks-label">Sort</span>
        <select bind:value={sort}>
          {#each SORTS as option (option.value)}<option value={option.value}>{option.label}</option>{/each}
        </select>
      </label>
      <label class="tasks-check">
        <input type="checkbox" bind:checked={waiting} />
        <span>Waiting on others</span>
      </label>
    </div>

    <p class="tasks-count" aria-live="polite">{countText}</p>

    {#if shown.length}
      <div class="tasks-list">
        {#each groups as group (group.path)}
          {#if sort === "document"}<h3 class="tasks-file">{group.path}</h3>{/if}
          <ul class="tasks-items">
            {#each group.tasks as task (`${task.path}:${task.from}`)}
              <li class="tasks-item" class:done={task.state === "done"} class:cancelled={task.state === "cancelled"}>
                <input
                  type="checkbox"
                  class="tasks-box"
                  use:box={task}
                  disabled={task.state === "cancelled"}
                  aria-label={`Done: ${task.text || "untitled task"}`}
                  onchange={(event) => {
                    onchange?.(task, "toggle");
                    showState(event.currentTarget, task);
                  }}
                />
                <div class="tasks-body">
                  <button type="button" class="tasks-text" onclick={() => onselect?.(task)}>
                    {task.text || "Untitled task"}
                  </button>
                  <div class="tasks-meta">
                    {#if task.state !== "todo"}<span class="chip">{STATE_LABEL[task.state] ?? task.state}</span>{/if}
                    {#if task.priority < 3}<span class="chip" title={PRIORITY_LABEL[task.priority]}>P{task.priority}</span>{/if}
                    {#if task.due}
                      {@const overdue = isOverdue(task.state, task.due, today)}
                      <span class="chip" class:overdue>{overdue ? "Overdue, " : "Due "}{shortDate(task.due, today)}</span>
                    {/if}
                    {#each assigneesOf(task) as name (name)}<span class="chip" title="Assigned to {name}">@{name}</span>{/each}
                    {#each task.tags as tagged (tagged.name)}<span class="chip">#{tagged.name}</span>{/each}
                    {#if task.blockedBy.length}<span class="chip">after {task.blockedBy.join(", ")}</span>{/if}
                    {#if task.warnings.length}
                      <span class="warning" title={task.warnings.join(" ")}>
                        <Icon name="triangle-alert" size={14} />
                        <span class="sr-only">Warning: {task.warnings.join(" ")}</span>
                      </span>
                    {/if}
                    {#if sort !== "document"}<span class="tasks-path">{task.path}</span>{/if}
                  </div>
                </div>
              </li>
            {/each}
          </ul>
        {/each}
      </div>
    {:else}
      <p class="tasks-empty">No tasks match.</p>
    {/if}
  {/if}
</section>

<style>
  .tasks { display: flex; flex: 1 1 auto; min-height: 0; flex-direction: column; overflow: hidden; }
  .tasks-filters {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * 2);
    border-bottom: 1px solid var(--color-divider);
  }
  .tasks-field { display: flex; min-width: 0; flex-direction: column; gap: calc(var(--spacing) * .5); }
  .tasks-search { flex: 1 1 100%; }
  .tasks-label { color: var(--color-text-secondary); font-size: var(--text-sm); }
  .tasks-field input, .tasks-field select { max-width: 100%; font: inherit; font-size: var(--text-sm); }
  .tasks-search input { width: 100%; }
  .tasks-check { display: flex; align-items: center; gap: var(--spacing); color: var(--color-text-secondary); font-size: var(--text-sm); }
  .tasks-count { flex: none; margin: 0; padding: var(--spacing) calc(var(--spacing) * 2); color: var(--color-text-secondary); font-size: var(--text-sm); font-variant-numeric: tabular-nums; }
  .tasks-list { flex: 1 1 auto; min-height: 0; overflow: auto; padding: 0 var(--spacing) calc(var(--spacing) * 2); }
  .tasks-file {
    margin: calc(var(--spacing) * 2) var(--spacing) var(--spacing);
    color: var(--color-text-secondary);
    font-size: var(--text-sm);
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .tasks-items { margin: 0; padding: 0; list-style: none; }
  .tasks-item {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: calc(var(--spacing) * 2);
    align-items: start;
    padding: calc(var(--spacing) * 1.5) var(--spacing);
    border-radius: var(--radius-base);
    font-size: var(--text-sm);
  }
  .tasks-item:hover { background: var(--color-divider); }
  .tasks-item.done .tasks-text, .tasks-item.cancelled .tasks-text { color: var(--color-text-secondary); text-decoration: line-through; }
  .tasks-box { margin: calc(var(--spacing) * .5) 0 0; accent-color: var(--color-brand); }
  .tasks-body { display: flex; min-width: 0; flex-direction: column; gap: calc(var(--spacing) * .75); }
  .tasks-text {
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    overflow-wrap: anywhere;
    cursor: pointer;
  }
  .tasks-text:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; border-radius: var(--radius-base); }
  .tasks-meta { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 1.5); }
  .chip {
    padding: 0 calc(var(--spacing) * 1.5);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-base);
    color: var(--color-text-secondary);
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
  }
  .chip.overdue { border-color: currentColor; color: var(--color-error-text); font-weight: 600; }
  .warning { display: inline-flex; color: var(--color-warning-text); }
  .tasks-path { color: var(--color-text-secondary); font-size: var(--text-sm); overflow-wrap: anywhere; }
  .tasks-empty { margin: auto; padding: calc(var(--spacing) * 4); color: var(--color-text-secondary); font-size: var(--text-sm); text-align: center; }
</style>
