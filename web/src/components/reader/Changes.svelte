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
  let authorFilter = $state("");
  let fileFilter = $state("");
  let runFilter = $state("");
  let idsFilter = $state([]);
  $effect(() => { idsFilter = ids; });
  const idSet = $derived(new Set(idsFilter));
  let feedback = $state("");
  // Ticking changes is a mode you enter, not the state the queue sits in. A
  // review is one change at a time; answering several at once is the rarer
  // thing, so it waits behind a menu item and takes the row of verbs with it.
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
  const filteredRows = $derived(orderedRows.filter((item) =>
    (!authorFilter || authorOf(item) === authorFilter) && (!fileFilter || pathOf(item) === fileFilter) && (!runFilter || item.run === runFilter) && (!idsFilter.length || idSet.has(item.id))));
  const selectedRows = $derived(filteredRows.filter((item) => checked.has(item.id)));
  const active = $derived(filteredRows.some((item) => item.id === expanded) ? expanded : filteredRows.some((item) => item.id === selected) ? selected : filteredRows[0]?.id || "");
  const derivedAuthors = $derived([...new Set(changes.map(authorOf))]);
  const derivedFiles = $derived(files.length ? files.map((file) => file.path || file.id).filter(Boolean) : [...new Set(changes.map(pathOf))]);
  const derivedRuns = $derived([...new Set(changes.map((item) => item.run).filter(Boolean))]);
  const rowFor = (id) => filteredRows.find((item) => item.id === id);
  /// Whether a decision is in flight; the bulk verbs are disabled while one
  /// is, so a second click cannot answer for rows already being answered for.
  const busy = $derived(deciding.size > 0);
  const showExtraFilters = $derived(derivedAuthors.length > 1 || derivedFiles.length > 1 || derivedRuns.length > 0);
  // The filter names that are in force, in the order the label gives them.
  const filterNames = $derived([
    authorFilter ? authorLabel({ author: authorFilter }) : "",
    fileFilter,
    runFilter ? runLabel(runFilter) : "",
    idsFilter.length ? "From this task" : "",
  ].filter(Boolean));
  const countLabel = $derived(filterNames.length
    ? `${filteredRows.length} of ${changes.length} pending · ${filterNames.join(" · ")}`
    : `${changes.length} pending`);
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

  /// Answering several changes at once: the ticked ones, or every pending
  /// change when the verb came from the Bulk actions menu.
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
  // Several ticked rows are a decision worth a second look; one row is not.
  function answerSelected(action) {
    if (selectedRows.length > 1) askBulk(action, selectedRows, "selected");
    else bulk(action, selectedRows);
  }
  function clearFilters() { authorFilter = ""; fileFilter = ""; runFilter = ""; idsFilter = []; }
  // Bulk actions holds what acts on many changes at once: the mode that lets a
  // reviewer choose a set, and the two verbs that answer for every pending
  // change whatever the filter shows.
  function chooseMenuItem(value) {
    if (value === "select") startSelecting();
    else if (value === "accept-all") askBulk("accept", orderedRows, "all");
    else if (value === "reject-all") askBulk("reject", orderedRows, "all");
  }
  function chooseFilter(value) {
    const [key, ...rest] = String(value).split(":");
    const answer = rest.join(":");
    idsFilter = [];
    if (key === "author") authorFilter = answer;
    else if (key === "file") fileFilter = answer;
    else if (key === "run") runFilter = answer;
  }
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
  <!-- Two rows and nothing else above the queue: whether subsequent edits are
       tracked, the count of what is pending, and the filter and bulk menus
       beside it, so the first reviewable change sits close to the top of the
       panel. -->
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
    <div class="changes-toolbar">
      {#if showExtraFilters}<Menu onSelect={(chosen) => chooseFilter(chosen.value)}>
        <Menu.Trigger>
          {#snippet element(attributes)}
            <button {...attributes} type="button" class="filter-trigger" aria-label="Which changes to show"><span class="filter-label">{countLabel}</span><Icon name="chevron-down" size="12px" /></button>
          {/snippet}
        </Menu.Trigger>
        <ExplorerMenu>
          {#if derivedAuthors.length > 1}
            <div class="changes-menu-label">Author</div>
            <Menu.Item value="author:" class="menuitem"><span class="menuitem-check">{authorFilter ? "" : "✓"}</span>Anyone</Menu.Item>
            {#each derivedAuthors as author}
              <Menu.Item value={`author:${author}`} class="menuitem"><span class="menuitem-check">{authorFilter === author ? "✓" : ""}</span>{authorLabel({ author })}</Menu.Item>
            {/each}
          {/if}
          {#if derivedFiles.length > 1}
            {#if derivedAuthors.length > 1}<hr class="hr my-1" />{/if}
            <div class="changes-menu-label">File</div>
            <Menu.Item value="file:" class="menuitem"><span class="menuitem-check">{fileFilter ? "" : "✓"}</span>Every file</Menu.Item>
            {#each derivedFiles as file}
              <Menu.Item value={`file:${file}`} class="menuitem"><span class="menuitem-check">{fileFilter === file ? "✓" : ""}</span>{file}</Menu.Item>
            {/each}
          {/if}
          {#if derivedRuns.length}
            {#if derivedAuthors.length > 1 || derivedFiles.length > 1}<hr class="hr my-1" />{/if}
            <div class="changes-menu-label">Run</div>
            <Menu.Item value="run:" class="menuitem"><span class="menuitem-check">{runFilter ? "" : "✓"}</span>Every run</Menu.Item>
            {#each derivedRuns as id}
              <Menu.Item value={`run:${id}`} class="menuitem"><span class="menuitem-check">{runFilter === id ? "✓" : ""}</span>{runLabel(id)}</Menu.Item>
            {/each}
          {/if}
        </ExplorerMenu>
      </Menu>{:else}<span class="changes-count">{countLabel}</span>{/if}
      <span class="changes-live sr-only" aria-live="polite">{changes.length} pending</span>
      {#if canReview && !selecting}<Menu onSelect={(chosen) => chooseMenuItem(chosen.value)} positioning={{ placement: "bottom-end" }}>
        <Menu.Trigger>
          {#snippet element(attributes)}
            <button {...attributes} type="button" class="bulk-trigger">Bulk actions<Icon name="chevron-down" size="12px" /></button>
          {/snippet}
        </Menu.Trigger>
        <ExplorerMenu positioning={{ placement: "bottom-end" }}>
          <Menu.Item value="select" class="menuitem" disabled={!filteredRows.length}><Icon name="list-checks" size="14px" />Select multiple</Menu.Item>
          <hr class="hr my-1" />
          <Menu.Item value="accept-all" class="menuitem" disabled={!changes.length}><Icon name="check-check" size="14px" />Accept all</Menu.Item>
          <Menu.Item value="reject-all" class="menuitem" disabled={!changes.length}><Icon name="x" size="14px" />Reject all</Menu.Item>
        </ExplorerMenu>
      </Menu>{/if}
    </div>
  </header>
  {#if idsFilter.length}<div class="ids-bar">
    <span class="filter-label">From this task</span>
    <button type="button" class="bar-action show-all" onclick={() => idsFilter = []}>Show all</button>
  </div>{/if}
  <!-- Ticking rows, and the verbs that go with it, for as long as the mode lasts. -->
  {#if selecting}<div class="select-bar" role="toolbar" aria-label="Selected changes">
    <span class="select-count" aria-live="polite">{checked.size} selected</span>
    <button type="button" class="bar-action select-all" disabled={filteredRows.every((item) => checked.has(item.id))} onclick={() => checked = new Set(filteredRows.map((item) => item.id))}>Select all</button>
    {#if canReview}<button type="button" class="bar-action reject" disabled={!selectedRows.length || busy} onclick={() => answerSelected("reject")}>Reject</button>
    <button type="button" class="bar-action accept" disabled={!selectedRows.length || busy} onclick={() => answerSelected("accept")}>Accept</button>{/if}
    <button type="button" class="bar-action cancel" onclick={stopSelecting}>Cancel</button>
  </div>{/if}
  {#if confirming}<div class="confirm-bar" role="alertdialog" aria-labelledby="changes-confirm-text">
    <span class="confirm-text" id="changes-confirm-text">{confirmText(confirming)}</span>
    <button type="button" class="bar-action confirm {confirming.action}" disabled={busy} onclick={() => bulk(confirming.action, confirming.rows)}>{confirmLabel(confirming)}</button>
    <button type="button" class="bar-action cancel" onclick={() => confirming = null}>Cancel</button>
  </div>{/if}
  {#if feedback}<p class="panel-status" role="status" aria-live="polite">{feedback}</p>{/if}
  {#if !filteredRows.length}<div class="changes-empty" role="status">
    {#if changes.length}
      <p class="changes-empty-title">No changes match this filter</p>
      <p class="changes-empty-note"><button type="button" class="clear-filters" onclick={clearFilters}>Show all changes</button></p>
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
  .changes-panel { display: flex; flex-direction: column; min-height: 0; outline: none; height: 100%; }
  /* Two rows of header, the count and the menus under the title, so the first
     change sits near the top. Both stay put while the queue scrolls beneath. */
  .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; padding: .5rem var(--spacing); background: var(--color-sidebar); }
  .changes-title-row { display: flex; flex-wrap: wrap; align-items: center; gap: .25rem .5rem; }
  h2 { margin: 0; flex: 1 1 auto; min-width: 0; }
  /* The track and thumb are `.switch` in librepaper.css, worn here and in the
     settings dialog alike; this is only where the word sits beside them. */
  .changes-panel :global(.tracking-toggle) { display: flex; align-items: center; gap: .35rem; flex: 0 0 auto; margin-left: auto; font-size: var(--panel-meta-size); white-space: nowrap; }
  .changes-toolbar { margin-top: .35rem; display: flex; flex-wrap: wrap; gap: .35rem .5rem; align-items: center; justify-content: space-between; }
  .changes-count { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  /* The filter and bulk triggers are the same kind of control: a small outlined
     button that takes the same hover as a row. */
  .filter-trigger, .bulk-trigger { display: inline-flex; align-items: center; gap: .3rem; min-height: 1.75rem; padding: .15rem .45rem; border: 1px solid var(--color-divider); border-radius: .25rem; background: none; color: inherit; font-size: var(--panel-font-size); font-weight: 500; cursor: pointer; }
  .filter-trigger:hover, .filter-trigger[data-state="open"], .bulk-trigger:hover, .bulk-trigger[data-state="open"] { background: var(--color-row-hover); }
  .filter-trigger { flex: 0 1 auto; min-width: 0; max-width: 100%; }
  .filter-trigger :global(svg), .bulk-trigger :global(svg) { color: var(--panel-muted); flex: 0 0 auto; }
  .filter-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .bulk-trigger { flex: 0 0 auto; margin-left: auto; white-space: nowrap; }
  .changes-menu-label { padding: .35rem .65rem .2rem; color: var(--panel-muted); font-size: var(--panel-meta-size); font-weight: 600; text-transform: uppercase; letter-spacing: .06em; }
  .ids-bar { flex: 0 0 auto; display: flex; flex-wrap: wrap; align-items: center; gap: .3rem .5rem; padding: .3rem var(--spacing); background: var(--color-sidebar); border-bottom: 1px solid var(--color-divider); font-size: var(--panel-meta-size); }
  .ids-bar .filter-label { flex: 1 1 auto; color: var(--panel-muted); }
  /* The contextual bars: only while a selection or a confirmation is open, and
     gone the moment it is answered or dismissed. */
  .select-bar, .confirm-bar { flex: 0 0 auto; display: flex; flex-wrap: wrap; align-items: center; gap: .3rem .4rem; padding: .4rem var(--spacing); border-bottom: 1px solid var(--color-divider); background: color-mix(in srgb, var(--color-brand) 6%, var(--color-sidebar)); }
  .select-count, .confirm-text { flex: 1 1 8rem; min-width: 0; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .row-action, .bar-action { display: inline-flex; align-items: center; gap: .25rem; min-height: 1.75rem; padding: .2rem .5rem; border: 1px solid transparent; border-radius: .25rem; background: none; color: inherit; font-size: var(--panel-meta-size); font-weight: 500; cursor: pointer; }
  .row-action:hover:not(:disabled), .bar-action:hover:not(:disabled) { background: var(--color-row-hover); }
  .row-action:disabled, .bar-action:disabled { opacity: .45; cursor: default; }
  .row-action.accept, .bar-action.accept { border-color: var(--color-divider); color: var(--color-success-text); }
  .bar-action.confirm { border-color: var(--color-divider); }
  .row-action:focus-visible, .bar-action:focus-visible, .bulk-trigger:focus-visible, .filter-trigger:focus-visible, .row-more:focus-visible, .clear-filters:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  /* Touch needs room for a thumb; a mouse does not. */
  @media (pointer: coarse) {
    .row-action, .bar-action, .bulk-trigger, .filter-trigger { min-height: 2.25rem; }
  }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; overscroll-behavior: contain; }
  /* A row is the change and a line saying whose it is. No card, no border but
     the hairline between one change and the next. */
  .change-row { position: relative; content-visibility: auto; contain-intrinsic-size: 0 3.4rem; padding: .6rem var(--spacing); border-bottom: 1px solid var(--color-divider); }
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
  .row-more, .clear-filters { margin-top: .2rem; padding: .1rem 0; border: 0; background: none; color: var(--color-link, var(--color-brand)); font-size: var(--panel-meta-size); cursor: pointer; }
  .row-actions { display: flex; justify-content: flex-end; gap: .35rem; margin-top: .4rem; }
  .changes-empty { padding: 1rem var(--spacing); }
  .changes-empty-title { margin: 0; font-weight: 500; font-size: var(--panel-font-size); }
  .changes-empty-note { margin: .2rem 0 0; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .changes-hint { flex: 0 0 auto; padding: .35rem var(--spacing); border-top: 1px solid var(--color-divider); color: var(--panel-muted); font-size: var(--panel-meta-size); text-align: center; }
</style>
