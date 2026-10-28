<script>
  import { Menu, Switch } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";

  // The pending changes of the document, one row each:
  // `{id, file_id, path, author, who, run, at, position, line, before, after}`.
  // Accepting or rejecting a change is an edit to the shared document, so the
  // callbacks answer when the edit is made.
  let {
    changes = [], files = [], tracking = false, canTrack = true, canReview = true,
    selected = null,
    // The ids to show first, when the reader sent the reviewer here from an
    // agent task. A task can make changes under several runs, so the whole
    // set of ids is the filter, not one run.
    ids = [],
    ontracking, onaccept, onreject, onrejectmany, onreveal,
    identity = "",
  } = $props();

  let expanded = $state("");
  let checked = $state(new Set());
  let deciding = $state(new Set());
  let expandedText = $state(new Set());
  // The one filter the queue shows: "all", "task", or "author:", "file:" or
  // "run:" followed by the value it picks.
  let scope = $state("all");
  $effect(() => { scope = ids.length ? "task" : "all"; });
  const idSet = $derived(new Set(ids));
  let feedback = $state("");
  // Ticking changes is a mode you enter, not the state the queue sits in. A
  // review is one change at a time; answering several at once is the rarer
  // thing, so it waits behind the Select button and the bar of verbs takes the
  // place of the header's toolbar while it lasts.
  let selecting = $state(false);
  // A bulk verb held for one confirmation: the action, the rows it answers for,
  // and whether those are every pending change or only the ticked ones.
  let confirming = $state(null);

  const authorOf = (item) => item.author || "Unknown author";
  const pathOf = (item) => item.path || "Untitled";
  const authorLabel = (item) => (identity && item.author === identity ? `${item.author} (you)` : authorOf(item));
  // A row names its own author as "You" so the reviewer sees at once whose
  // changes they are answering for.
  const whoLabel = (item) => (identity && item.author === identity ? "You" : authorOf(item));
  const runLabel = (id) => `Run ${String(id).slice(0, 8)}`;
  const kindOf = (item) => (item.before && item.after ? "Replacement" : item.after ? "Addition" : "Deletion");
  // A row with no offset sorts last within its file.
  const positionOf = (item) => (Number.isFinite(item.position) ? item.position : Number.MAX_SAFE_INTEGER);
  // A change this long shows its first six lines until the reviewer asks for
  // the rest, so one large paste cannot push the queue out of view.
  const isLong = (item) => [item.before, item.after].some((text) => (text || "").length > 280 || (text || "").split("\n").length > 6);
  // A scope string split into its kind and the value after the first colon.
  const scopeOf = (value) => { const [key, ...rest] = String(value).split(":"); return { key, answer: rest.join(":") }; };

  // session.list() is the stable file order. File identity wins over path so
  // renames do not reshuffle an existing review queue.
  const fileOrder = $derived.by(() => {
    const order = new Map();
    files.forEach((file, index) => {
      for (const key of [file?.id, file?.path]) if (key !== "" && key !== undefined && !order.has(String(key))) order.set(String(key), index);
    });
    return order;
  });
  const orderOf = (item) => fileOrder.get(String(item.file_id || pathOf(item))) ?? Number.MAX_SAFE_INTEGER;
  const orderedRows = $derived([...changes].sort((a, b) => orderOf(a) - orderOf(b) || pathOf(a).localeCompare(pathOf(b)) || positionOf(a) - positionOf(b) || (a.at || 0) - (b.at || 0) || a.id.localeCompare(b.id)));
  // Whether a row falls inside the scope the reviewer chose.
  function inScope(item) {
    if (scope === "all") return true;
    if (scope === "task") return idSet.has(item.id);
    const { key, answer } = scopeOf(scope);
    if (key === "author") return authorOf(item) === answer;
    if (key === "file") return pathOf(item) === answer;
    return key === "run" && item.run === answer;
  }
  const filteredRows = $derived(orderedRows.filter(inScope));
  const selectedRows = $derived(filteredRows.filter((item) => checked.has(item.id)));
  const active = $derived(filteredRows.some((item) => item.id === expanded) ? expanded : filteredRows.some((item) => item.id === selected) ? selected : filteredRows[0]?.id || "");
  const derivedAuthors = $derived([...new Set(changes.map(authorOf))]);
  const derivedFiles = $derived(files.length ? files.map((file) => file.path || file.id).filter(Boolean) : [...new Set(changes.map(pathOf))]);
  const derivedRuns = $derived([...new Set(changes.map((item) => item.run).filter(Boolean))]);
  const rowFor = (id) => filteredRows.find((item) => item.id === id);
  /// Whether a decision is in flight; the bulk verbs are disabled while one
  /// is, so a second click cannot answer for rows already being answered for.
  const busy = $derived(deciding.size > 0);
  // Every pending change ticked in the all scope: the verbs then read "Accept
  // all" and "Reject all", and answering them asks first.
  const allChosen = $derived(scope === "all" && changes.length > 0 && selectedRows.length === changes.length);
  // The scopes the menu offers: all changes, this task when there are ids,
  // then one entry per author, file and run, each under its group's heading.
  const scopes = $derived.by(() => {
    const list = [{ value: "all", label: "All changes", group: "" }];
    if (ids.length) list.push({ value: "task", label: "This task", group: "" });
    if (derivedAuthors.length > 1) for (const author of derivedAuthors) list.push({ value: `author:${author}`, label: authorLabel({ author }), group: "Author" });
    if (derivedFiles.length > 1) for (const file of derivedFiles) list.push({ value: `file:${file}`, label: file, group: "File" });
    for (const id of derivedRuns) list.push({ value: `run:${id}`, label: runLabel(id), group: "Run" });
    return list;
  });
  // The name of a scope whose option is gone, because its last change was
  // answered, is rebuilt from the scope string itself.
  const labelOf = (value) => {
    const { key, answer } = scopeOf(value);
    if (key === "author") return authorLabel({ author: answer });
    if (key === "file") return answer;
    if (key === "run") return runLabel(answer);
    return value === "task" ? "This task" : "All changes";
  };
  const scopeName = $derived(scopes.find((option) => option.value === scope)?.label ?? labelOf(scope));
  // How an empty queue names the scope it is empty for.
  const scopePhrase = $derived({
    task: "in this task",
    author: `from ${scopeName}`,
    file: `in ${scopeName}`,
    run: "in this run",
  }[scopeOf(scope).key] ?? "");
  /// The place a change sits, when the reviewer cannot tell it from the diff:
  /// the filename once several files are under review, and the line once the
  /// change has one.
  const locationOf = (item) => {
    const line = Number.isFinite(item.line) ? item.line : null;
    if (derivedFiles.length > 1) return line === null ? pathOf(item) : `${pathOf(item)}:${line}`;
    return line === null ? "" : `Line ${line}`;
  };

  $effect(() => {
    const valid = new Set(filteredRows.map((item) => item.id));
    const next = new Set([...checked].filter((id) => valid.has(id)));
    if (next.size !== checked.size) checked = next;
  });
  $effect(() => { if (expanded && !filteredRows.some((item) => item.id === expanded)) expanded = ""; });
  $effect(() => { if (selecting && (!canReview || !filteredRows.length)) stopSelecting(); });

  function startSelecting() { selecting = true; confirming = null; }
  function stopSelecting() { selecting = false; checked = new Set(); }
  function focusRow(id) { requestAnimationFrame(() => document.getElementById(`change-${id}`)?.focus({ preventScroll: true })); }
  function activate(item, { focus = false } = {}) {
    if (!item) return;
    expanded = item.id;
    onreveal?.(item);
    if (focus) focusRow(item.id);
  }
  function reveal(item) { activate(item, { focus: true }); }
  function toggleChecked(item) { const next = new Set(checked); next.has(item.id) ? next.delete(item.id) : next.add(item.id); checked = next; }
  function toggleText(item) { const next = new Set(expandedText); next.has(item.id) ? next.delete(item.id) : next.add(item.id); expandedText = next; }
  function decisionPromise(item, action) {
    const callback = action === "accept" ? onaccept : onreject;
    if (!callback) return Promise.reject(new Error("Review action is unavailable."));
    return callback(item);
  }
  function addBusy(id) { deciding = new Set([...deciding, id]); }
  function removeBusy(id) { deciding = new Set([...deciding].filter((current) => current !== id)); }
  function nextAfter(ids, oldIndex) { return [...ids.slice(oldIndex + 1), ...ids.slice(0, oldIndex).reverse()].map(rowFor).find(Boolean); }

  async function decide(item, action) {
    if (!canReview || deciding.has(item.id)) return;
    addBusy(item.id); feedback = "";
    const ids = filteredRows.map((row) => row.id); const oldIndex = ids.indexOf(item.id);
    try {
      await decisionPromise(item, action);
      // The row is gone once the document has changed; the next one is read
      // from the order the queue had before.
      const next = nextAfter(ids, oldIndex);
      if (next) activate(next, { focus: true }); else {
        expanded = "";
        requestAnimationFrame(() => {
          const panel = document.querySelector(".changes-panel");
          if (panel instanceof HTMLElement) panel.focus();
        });
      }
    } catch (error) { feedback = error?.message || `Could not ${action} this change.`; focusRow(item.id); }
    finally { removeBusy(item.id); }
  }

  /// Answering several changes at once: the rows it is handed, which are the
  /// ticked ones or the whole queue when every change is ticked.
  async function bulk(action, rows = selectedRows) {
    if (!canReview) return;
    // Take the rows before sending anything: the ticks are cleared and the
    // queue re-sorts as answers arrive, so this is a list and not a filter
    // that keeps being asked.
    const captured = [...rows]; if (!captured.length) return;
    captured.forEach((item) => addBusy(item.id)); stopSelecting(); confirming = null;
    try {
      // A bulk reject goes through one call, because rejecting a change that
      // swallowed others can split its owner and the order matters.
      if (action === "reject" && onrejectmany) {
        const count = Number(await onrejectmany(captured)) || 0;
        feedback = `${count} change${count === 1 ? "" : "s"} rejected.`;
        return;
      }
      const settled = await Promise.allSettled(captured.map((item) => decisionPromise(item, action)));
      const succeeded = settled.filter((entry) => entry.status === "fulfilled").length;
      const failed = settled.length - succeeded;
      feedback = failed ? `${succeeded} ${action}ed; ${failed} failed.` : `${succeeded} change${succeeded === 1 ? "" : "s"} ${action}ed.`;
    } catch (error) { feedback = error?.message || `Could not ${action} the selected changes.`; }
    finally { captured.forEach((item) => removeBusy(item.id)); }
  }
  // A bulk verb waits for a confirmation, and the confirm button takes focus
  // once the bar is drawn, so Enter answers it and Escape walks away.
  function askBulk(action, rows, scope) {
    confirming = { action, rows: [...rows], scope };
    requestAnimationFrame(() => {
      const button = document.querySelector(".confirm-bar .confirm");
      if (button instanceof HTMLElement) button.focus();
    });
  }
  const verbOf = (action) => (action === "accept" ? "Accept" : "Reject");
  const confirmText = ({ action, rows, scope }) => {
    const noun = rows.length === 1 ? "change" : "changes";
    return scope === "all"
      ? `${verbOf(action)} all ${rows.length} pending ${noun} in this project?`
      : `${verbOf(action)} ${rows.length} selected ${noun}?`;
  };
  const confirmLabel = ({ action, rows }) => `${verbOf(action)} ${rows.length}`;
  // Every ticked change in the all scope is the whole project and asks first,
  // as does any other set of several; one ticked row answers directly.
  function answerSelected(action) {
    if (allChosen) askBulk(action, selectedRows, "all");
    else if (selectedRows.length > 1) askBulk(action, selectedRows, "selected");
    else bulk(action, selectedRows);
  }
  function chooseScope(value) { scope = value; }
  function move(offset) { const index = filteredRows.findIndex((item) => item.id === active); const item = filteredRows[index + offset] || filteredRows[index]; if (item) activate(item, { focus: true }); }
  function keydown(event) {
    if (event.defaultPrevented || event.isComposing || event.target.closest("input,select,textarea,[contenteditable=true],details,[data-scope='menu']")) return;
    const item = rowFor(active); const key = event.key.toLowerCase();
    // The bar closes first; a second Escape leaves the selection it came from.
    if (event.key === "Escape" && confirming) { event.preventDefault(); confirming = null; return; }
    if (event.key === "Escape" && selecting) { event.preventDefault(); stopSelecting(); return; }
    if (event.key === "ArrowDown" || key === "j") { event.preventDefault(); move(1); }
    else if (event.key === "ArrowUp" || key === "k") { event.preventDefault(); move(-1); }
    else if (item && key === "a" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "accept"); }
    else if (item && key === "r" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "reject"); }
  }
</script>

{#snippet rowMain(item, isActive)}
  <button id={`change-${item.id}`} type="button" class="row-main" aria-current={isActive ? "true" : undefined} onclick={() => reveal(item)}>
    <span class="row-meta"><span class="row-kind">{kindOf(item)}</span><span class="row-who"><span class="row-author">{whoLabel(item)}</span>{#if item.run}<span class="row-run">{runLabel(item.run)}</span>{/if}</span></span>
    {#if locationOf(item)}<span class="row-location">{locationOf(item)}</span>{/if}
    <span class="row-diff" class:clamped={isLong(item) && !expandedText.has(item.id)}>{#if item.before}<span class="deletion"><span class="sign" aria-hidden="true">−</span><del class="change-text">{item.before}</del></span>{/if}{#if item.after}<span class="insertion"><span class="sign" aria-hidden="true">+</span><ins class="change-text">{item.after}</ins></span>{/if}</span>
  </button>
{/snippet}

<!-- A focusable review context keeps shortcuts out of source and discussion fields. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="panel changes-panel" role="application" tabindex="0" onkeydown={keydown} aria-label="Changes review" aria-keyshortcuts="ArrowDown ArrowUp J K A R">
  <!-- The header is the title row, then one bar: the scope menu with the count,
       or the selection or confirmation bar while either is open. Nothing else
       sits above the queue, so the first reviewable change is near the top. -->
  <header class="changes-header">
    <div class="changes-title-row">
      <h2 class="panel-section-title">Changes</h2>
      <!-- A real switch rather than a checkbox with a switch painted over it:
           the state is the control's own, so the name no longer has to carry
           "On" or "Off" for a screen reader to read it out. -->
      {#if canReview}
      <Switch class="tracking-toggle" checked={tracking} disabled={!canTrack}
        title={canTrack ? "Track subsequent edits in this document" : "Editing is unavailable"}
        onCheckedChange={({ checked }) => ontracking?.(checked)}>
        <Switch.Label>Track</Switch.Label>
        <Switch.Control class="switch"><Switch.Thumb class="switch-thumb" /></Switch.Control>
        <Switch.HiddenInput />
      </Switch>
      {/if}
    </div>
    {#if confirming}<div class="confirm-bar" role="alertdialog" aria-labelledby="changes-confirm-text">
      <p class="confirm-text" id="changes-confirm-text">{confirmText(confirming)}</p>
      <div class="bar-buttons">
        <button type="button" class="bar-action cancel" onclick={() => confirming = null}>Cancel</button>
        <button type="button" class="bar-action confirm {confirming.action}" disabled={busy} onclick={() => bulk(confirming.action, confirming.rows)}>{confirmLabel(confirming)}</button>
      </div>
    </div>
    {:else if selecting}<div class="select-bar" role="toolbar" aria-label="Selected changes">
      <div class="bar-row">
        <span class="select-count" aria-live="polite">{checked.size} selected</span>
        <div class="bar-end">
          <button type="button" class="text-action select-all" disabled={filteredRows.every((item) => checked.has(item.id))} onclick={() => checked = new Set(filteredRows.map((item) => item.id))}>Select all</button>
          <button type="button" class="text-action cancel" onclick={stopSelecting}>Cancel</button>
        </div>
      </div>
      <div class="bar-buttons">
        <button type="button" class="bar-action reject" disabled={!selectedRows.length || busy} onclick={() => answerSelected("reject")}>{allChosen ? "Reject all" : "Reject"}</button>
        <button type="button" class="bar-action accept" disabled={!selectedRows.length || busy} onclick={() => answerSelected("accept")}>{allChosen ? "Accept all" : "Accept"}</button>
      </div>
    </div>
    {:else if changes.length}<div class="changes-toolbar">
      {#if scopes.length > 1}<Menu onSelect={(chosen) => chooseScope(chosen.value)}>
        <Menu.Trigger>
          {#snippet element(attributes)}
            <button {...attributes} type="button" class="scope-trigger" aria-label={`Which changes to show: ${scopeName}`}><span class="scope-label">{scopeName}</span><Icon name="chevron-down" size="12px" /></button>
          {/snippet}
        </Menu.Trigger>
        <ExplorerMenu>
          {#each scopes.filter((option) => !option.group) as option (option.value)}
            <Menu.Item value={option.value} class="menuitem"><span class="menuitem-check">{scope === option.value ? "✓" : ""}</span>{option.label}</Menu.Item>
          {/each}
          {#each ["Author", "File", "Run"] as group}
            {#if scopes.some((option) => option.group === group)}
              <hr class="hr my-1" />
              <div class="changes-menu-label">{group}</div>
              {#each scopes.filter((option) => option.group === group) as option (option.value)}
                <Menu.Item value={option.value} class="menuitem"><span class="menuitem-check">{scope === option.value ? "✓" : ""}</span>{option.label}</Menu.Item>
              {/each}
            {/if}
          {/each}
        </ExplorerMenu>
      </Menu>{:else}<span class="scope-label scope-static">All changes</span>{/if}
      <div class="toolbar-end">
        <span class="changes-count">{filteredRows.length} pending</span>
        {#if canReview && filteredRows.length}<button type="button" class="text-action select-start" onclick={startSelecting}>Select</button>{/if}
      </div>
    </div>{/if}
    <span class="changes-live sr-only" aria-live="polite">{filteredRows.length} pending</span>
  </header>
  {#if feedback}<p class="panel-status" role="status" aria-live="polite">{feedback}</p>{/if}
  {#if !filteredRows.length}<div class="changes-empty" role="status">
    {#if changes.length}
      <p class="changes-empty-title">No pending changes {scopePhrase}</p>
      <p class="changes-empty-note">{changes.length} pending in all changes.</p>
    {:else}
      <p class="changes-empty-title">No pending changes</p>
      <p class="changes-empty-note">Changes to review will appear here.</p>
    {/if}
  </div>{/if}
  <div class="changes-list" role="list" aria-label="Revision queue">
    {#each filteredRows as item (item.id)}
      {@const isActive = active === item.id}
      <article class="change-row" class:active={isActive} class:picked={checked.has(item.id)} role="listitem">
        {#if selecting}<div class="row-head"><input type="checkbox" class="row-pick" checked={checked.has(item.id)} aria-label={`Select ${kindOf(item).toLowerCase()} by ${whoLabel(item)}`} onclick={(event) => event.stopPropagation()} onchange={() => toggleChecked(item)} />{@render rowMain(item, isActive)}</div>{:else}{@render rowMain(item, isActive)}{/if}
        {#if isLong(item)}<button type="button" class="row-more" aria-expanded={expandedText.has(item.id)} onclick={() => toggleText(item)}>{expandedText.has(item.id) ? "Show less" : "Show more"}</button>{/if}
        <!-- Every change offers its answer, so a reviewer can act on any row
             without first selecting it. -->
        {#if canReview && !selecting}<div class="row-actions">
          <button type="button" class="row-action reject" aria-label={`Reject this ${kindOf(item).toLowerCase()}`} title="Reject this change" disabled={deciding.has(item.id)} onclick={() => void decide(item, "reject")}><Icon name="x" size="14px" />{deciding.has(item.id) ? "Rejecting…" : "Reject"}</button>
          <button type="button" class="row-action accept" aria-label={`Accept this ${kindOf(item).toLowerCase()}`} title="Accept this change" disabled={deciding.has(item.id)} onclick={() => void decide(item, "accept")}><Icon name="check" size="14px" />{deciding.has(item.id) ? "Accepting…" : "Accept"}</button>
        </div>{/if}
      </article>
    {/each}
  </div>
  <!-- The shortcuts, said once and quietly, rather than a panel of
       documentation standing where changes could be. -->
  {#if filteredRows.length}<footer class="changes-hint">J/K next · A accept · R reject</footer>{/if}
</div>

<style>
  .changes-panel { display: flex; flex-direction: column; height: 100%; min-height: 0; padding: 0; outline: none; container-type: inline-size; }
  /* The header holds the title and the tracking switch, then one bar. It stays
     put while the queue scrolls beneath, and its side padding is the panel's
     gutter, so the title, the bars and the rows share one left edge. */
  .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; display: flex; flex-direction: column; gap: .25rem; padding: 0 var(--panel-padding) .75rem; border-bottom: 1px solid var(--color-divider); background: var(--color-sidebar); }
  .changes-title-row { display: flex; align-items: center; justify-content: space-between; gap: .5rem; min-height: 2.5rem; }
  h2 { margin: 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* The track and thumb are `.switch` in librepaper.css, worn here and in the
     settings dialog alike; this is only where the word sits beside them. */
  .changes-panel :global(.tracking-toggle) { display: inline-flex; align-items: center; gap: .5rem; flex: none; min-height: 2rem; color: var(--panel-muted); font-size: var(--panel-font-size); white-space: nowrap; cursor: pointer; }
  .changes-toolbar, .bar-row { display: flex; align-items: center; justify-content: space-between; gap: .5rem; min-width: 0; min-height: 2rem; }
  /* The scope menu's trigger is an outlined control that takes the same hover
     as a row. */
  .scope-trigger { display: inline-flex; align-items: center; gap: .25rem; flex: 0 1 auto; min-width: 0; min-height: 2rem; padding: 0 .5rem; border: 1px solid var(--color-divider); border-radius: var(--radius-base); background: none; color: inherit; font-size: var(--panel-font-size); font-weight: 400; cursor: pointer; }
  .scope-trigger:hover, .scope-trigger[data-state="open"] { background: var(--color-row-hover); }
  .scope-trigger :global(svg) { flex: none; color: var(--panel-muted); }
  .scope-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .scope-static { font-size: var(--panel-font-size); }
  .toolbar-end, .bar-end { display: inline-flex; align-items: center; gap: .75rem; flex: none; white-space: nowrap; }
  .changes-count { color: var(--panel-muted); font-size: var(--panel-meta-size); }
  /* In a column this narrow the scope name and Select keep the room and the
     count gives it up; the live region still reads it out. */
  @container (max-width: 16.5rem) { .changes-count { display: none; } }
  .select-count { font-size: var(--panel-font-size); font-weight: 500; }
  /* Text actions read as links; the bars' own buttons are the bordered ones. */
  .text-action { display: inline-flex; align-items: center; min-height: 2rem; padding: 0; border: 0; background: none; color: var(--color-link, var(--color-brand)); font-size: var(--panel-meta-size); font-weight: 500; cursor: pointer; }
  .text-action:hover:not(:disabled) { text-decoration: underline; }
  .text-action:disabled { color: var(--panel-muted); cursor: default; }
  .select-bar, .confirm-bar { display: flex; flex-direction: column; gap: .5rem; }
  .confirm-text { margin: 0; min-height: 2rem; display: flex; align-items: center; font-size: var(--panel-font-size); line-height: var(--panel-line-height); }
  .bar-buttons { display: grid; grid-template-columns: 1fr 1fr; gap: .5rem; }
  .bar-action, .row-action { display: inline-flex; align-items: center; justify-content: center; gap: .25rem; min-height: 2rem; padding: 0 .75rem; border: 1px solid var(--color-divider); border-radius: var(--radius-base); background: none; color: inherit; font-size: var(--panel-meta-size); font-weight: 500; cursor: pointer; white-space: nowrap; }
  .row-action { min-height: 1.75rem; padding: 0 .5rem; }
  .bar-action:hover:not(:disabled), .row-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .bar-action:disabled, .row-action:disabled { opacity: .45; cursor: default; }
  .bar-action.accept, .row-action.accept { color: var(--color-success-text); }
  :is(.scope-trigger, .text-action, .bar-action, .row-action, .row-more):focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  /* Touch needs room for a thumb; a mouse does not. */
  @media (pointer: coarse) { .scope-trigger, .text-action, .bar-action, .row-action { min-height: 2.25rem; } }
  .changes-menu-label { padding: .35rem .65rem .2rem; color: var(--panel-muted); font-size: var(--panel-meta-size); font-weight: 600; text-transform: uppercase; letter-spacing: .06em; }
  .panel-status { margin: 0; padding: .5rem var(--panel-padding) 0; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; overscroll-behavior: contain; }
  /* A row is the change and a line saying whose it is. No card, no border but
     the hairline between one change and the next. */
  .change-row { position: relative; content-visibility: auto; contain-intrinsic-size: 0 3.4rem; padding: .75rem var(--panel-padding); border-bottom: 1px solid var(--color-divider); }
  .change-row:hover { background: var(--color-row-hover); }
  /* The row under review is tinted and marked with a thin bar on its left edge. */
  .change-row.active { background: color-mix(in srgb, var(--color-brand) 7%, transparent); box-shadow: inset 2px 0 0 var(--color-brand); }
  .change-row.picked { background: color-mix(in srgb, var(--color-brand) 10%, transparent); }
  .row-head { display: flex; align-items: flex-start; gap: .5rem; }
  .row-pick { flex: none; margin-top: .2rem; width: 1rem; height: 1rem; }
  .row-main { display: block; flex: 1; min-width: 0; width: 100%; padding: 0; border: 0; background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
  .row-main:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; border-radius: .2rem; }
  .row-meta { display: flex; gap: .5rem; overflow: hidden; color: var(--panel-muted); font-size: var(--panel-meta-size); white-space: nowrap; }
  .row-kind { flex: 0 0 auto; font-weight: 500; }
  .row-who { min-width: 0; margin-left: auto; overflow: hidden; text-overflow: ellipsis; }
  .row-run::before { content: "\00b7"; margin-inline: .3rem; }
  .row-location { display: block; margin-top: .1rem; overflow: hidden; color: var(--panel-muted); font-size: var(--panel-meta-size); text-overflow: ellipsis; white-space: nowrap; }
  /* The diff quotes the source, so it wears the source's monospace at the
     panel's own size rather than a hand-set one. */
  .row-diff { display: block; margin-top: .35rem; font-family: var(--font-editor); font-size: var(--panel-font-size); line-height: var(--panel-line-height); white-space: pre-wrap; overflow-wrap: anywhere; }
  .insertion, .deletion { display: block; padding: .1rem .35rem; border-radius: .2rem; }
  .deletion + .insertion { margin-top: .15rem; }
  .insertion { color: var(--color-success-text); background: color-mix(in srgb, var(--color-success-solid) 8%, transparent); }
  .deletion { color: var(--color-error-text); background: color-mix(in srgb, var(--color-error-solid) 7%, transparent); }
  del.change-text { text-decoration: line-through; text-decoration-color: color-mix(in srgb, currentColor 55%, transparent); }
  ins.change-text { text-decoration: none; }
  .sign { margin-right: .35rem; font-weight: 600; user-select: none; }
  .row-diff.clamped .insertion, .row-diff.clamped .deletion { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 6; line-clamp: 6; overflow: hidden; }
  .row-more { margin-top: .2rem; padding: .1rem 0; border: 0; background: none; color: var(--color-link, var(--color-brand)); font-size: var(--panel-meta-size); cursor: pointer; }
  .row-actions { display: flex; justify-content: flex-end; gap: .5rem; margin-top: .5rem; }
  .changes-empty { padding: 2rem var(--panel-padding); text-align: center; text-wrap: balance; }
  .changes-empty-title { margin: 0; font-weight: 500; font-size: var(--panel-font-size); }
  .changes-empty-note { margin: .2rem 0 0; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .changes-hint { flex: 0 0 auto; padding: .5rem var(--panel-padding); border-top: 1px solid var(--color-divider); color: var(--panel-muted); font-size: var(--panel-meta-size); text-align: center; }
</style>
