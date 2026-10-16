<script>
  import { Menu, Switch } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";

  // The pending changes of the document, one row each:
  // `{id, file_id, path, author, who, run, at, position, before, after}`.
  // Accepting or rejecting a change is an edit to the shared document, so the
  // callbacks answer when the edit is made.
  let {
    changes = [], files = [], tracking = false, canTrack = true, canReview = true,
    selected = null,
    // The ids to show first, when the reader sent the reviewer here from an
    // agent task. A task can make changes under several runs, so the whole
    // set of ids is the filter, not one run.
    ids = [],
    ontracking, onaccept, onreject, onreveal,
    identity = "",
  } = $props();

  let expanded = $state("");
  let checked = $state(new Set());
  let deciding = $state(new Set());
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
  // A bulk verb aimed at everything on show, held for one confirmation.
  let confirming = $state("");

  const authorOf = (item) => item.author || "Unknown author";
  const pathOf = (item) => item.path || "Untitled";
  const authorLabel = (item) => (identity && item.author === identity ? `${item.author} (you)` : authorOf(item));
  const runLabel = (id) => `Run ${String(id).slice(0, 8)}`;
  // A row with no offset sorts last within its file.
  const positionOf = (item) => (Number.isFinite(item.position) ? item.position : Number.MAX_SAFE_INTEGER);

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
  /// Whether ticking a row leads anywhere: to a decision about several
  /// changes at once. Without one the checkbox does nothing, so it is not drawn.
  const canPick = $derived(canReview && filteredRows.length > 1);
  const showExtraFilters = $derived(derivedAuthors.length > 1 || derivedFiles.length > 1 || derivedRuns.length > 0);
  /// Whether the filename belongs on the metadata line. One file under review
  /// names itself in every row, which is a word that tells the reviewer
  /// nothing; two or more and it is the only thing placing the change.
  const showPath = $derived(derivedFiles.length > 1);
  const filterLabel = $derived([
    "Pending changes",
    authorFilter ? authorLabel({ author: authorFilter }) : "",
    fileFilter,
    runFilter ? runLabel(runFilter) : "",
    idsFilter.length ? "From this task" : "",
  ].filter(Boolean).join(" · "));
  /// The ••• appears only when it holds something: an empty menu is a button
  /// that opens nothing.
  const hasMenu = $derived(canPick || (canReview && filteredRows.length > 0));

  $effect(() => {
    const valid = new Set(filteredRows.map((item) => item.id));
    const next = new Set([...checked].filter((id) => valid.has(id)));
    if (next.size !== checked.size) checked = next;
  });
  $effect(() => { if (expanded && !filteredRows.some((item) => item.id === expanded)) expanded = ""; });
  $effect(() => { if (selecting && !canPick) stopSelecting(); });

  function startSelecting() { selecting = true; confirming = ""; }
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

  /// Answering several changes at once: the ticked ones, or everything on show
  /// when the verb came from the menu rather than from a selection.
  async function bulk(action, rows = selectedRows) {
    if (!canReview) return;
    // Take the rows before sending anything: the ticks are cleared and the
    // queue re-sorts as answers arrive, so this is a list and not a filter
    // that keeps being asked.
    const captured = [...rows]; if (!captured.length) return;
    captured.forEach((item) => addBusy(item.id)); stopSelecting(); confirming = "";
    try {
      const settled = await Promise.allSettled(captured.map((item) => decisionPromise(item, action)));
      const succeeded = settled.filter((entry) => entry.status === "fulfilled").length;
      const failed = settled.length - succeeded;
      feedback = failed ? `${succeeded} ${action}ed; ${failed} failed.` : `${succeeded} change${succeeded === 1 ? "" : "s"} ${action}ed.`;
    } catch (error) { feedback = error?.message || `Could not ${action} the selected changes.`; }
    finally { captured.forEach((item) => removeBusy(item.id)); }
  }
  // The menu is where the infrequent things live: the two verbs that answer
  // for everything on show, and the mode that lets a reviewer choose a set.
  // Neither belongs in the panel itself, where they would stand above every
  // review as controls nobody presses.
  function chooseMenuItem(value) {
    if (value === "select") startSelecting();
    else if (value === "accept-all") confirming = "accept";
    else if (value === "reject-all") confirming = "reject";
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
    if (event.key === "Escape" && (selecting || confirming)) { event.preventDefault(); stopSelecting(); confirming = ""; return; }
    if (event.key === "ArrowDown" || key === "j") { event.preventDefault(); move(1); }
    else if (event.key === "ArrowUp" || key === "k") { event.preventDefault(); move(-1); }
    else if (item && key === "a" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "accept"); }
    else if (item && key === "r" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "reject"); }
  }
</script>

<!-- A focusable review context keeps shortcuts out of source and discussion fields. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="panel changes-panel" role="application" tabindex="0" onkeydown={keydown} aria-label="Changes review" aria-keyshortcuts="ArrowDown ArrowUp J K A R">
  <!-- Two rows and nothing else above the queue: what this is, whether
       subsequent edits are tracked, and which changes are on show. Everything
       a reviewer does rarely is behind the ••• beside them, so the first
       reviewable change is a finger's width from the top of the panel rather
       than a screen down it. -->
  <header class="changes-header">
    <div class="changes-title-row">
      <h2 class="panel-section-title">Changes</h2>
      <div class="title-controls">
        <!-- A real switch rather than a checkbox with a switch painted over it:
             the state is the control's own, so the name no longer has to carry
             "On" or "Off" for a screen reader to read it out. -->
        {#if canReview}
        <Switch class="tracking-toggle" checked={tracking} disabled={!canTrack}
          title={canTrack ? "Track subsequent edits in this document" : "Editing is unavailable"}
          onCheckedChange={({ checked }) => ontracking?.(checked)}>
          <Switch.Label>Track changes</Switch.Label>
          <Switch.Control class="switch"><Switch.Thumb class="switch-thumb" /></Switch.Control>
          <Switch.HiddenInput />
        </Switch>
        {/if}
        {#if hasMenu}<Menu onSelect={(chosen) => chooseMenuItem(chosen.value)}>
          <!-- The button is authored here rather than handed a `class`: a class
               arriving as a prop carries no scope hash, so the rules below would
               have to be global to paint at all. -->
          <Menu.Trigger>
            {#snippet element(attributes)}
              <button {...attributes} class="changes-menu" aria-label="More change options">•••</button>
            {/snippet}
          </Menu.Trigger>
          <ExplorerMenu>
            {#if canPick}<Menu.Item value="select" class="menuitem">Select multiple</Menu.Item>{/if}
            {#if canReview && filteredRows.length}
              <Menu.Item value="accept-all" class="menuitem">Accept all pending changes</Menu.Item>
              <Menu.Item value="reject-all" class="menuitem">Reject all pending changes</Menu.Item>
            {/if}
          </ExplorerMenu>
        </Menu>{/if}
      </div>
    </div>
    <div class="changes-meta" aria-live="polite">{changes.length} pending{filteredRows.length !== changes.length ? ` · ${filteredRows.length} shown` : ""}</div>
  </header>
  {#if idsFilter.length}<div class="filter-bar ids-bar">
    <span class="filter-label">From this task</span>
    <button type="button" class="bar-action show-all" onclick={() => idsFilter = []}>Show all</button>
  </div>{/if}
  <!-- One control, only visible when there are several authors or files, or any run, to filter by. -->
  {#if showExtraFilters}<div class="filter-bar">
    <Menu onSelect={(chosen) => chooseFilter(chosen.value)}>
      <Menu.Trigger>
        {#snippet element(attributes)}
          <button {...attributes} type="button" class="filter-trigger" aria-label="Which changes to show"><Icon name="list-filter" size="14px" /><span class="filter-label">{filterLabel}</span><Icon name="chevron-down" size="12px" /></button>
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
    </Menu>
  </div>{/if}
  <!-- Ticking rows, and the verbs that go with it, for as long as the mode lasts. -->
  {#if selecting}<div class="select-bar" role="group" aria-label="Selected changes">
    <span class="select-count">{checked.size} selected</span>
    {#if canReview}<button type="button" class="bar-action reject" disabled={!selectedRows.length || busy} onclick={() => bulk("reject")}>Reject</button>
    <button type="button" class="bar-action accept" disabled={!selectedRows.length || busy} onclick={() => bulk("accept")}>Accept</button>{/if}
    <button type="button" class="bar-action" onclick={stopSelecting}>Cancel</button>
  </div>{/if}
  {#if confirming}<div class="select-bar confirm-bar" role="group" aria-label="Confirm a decision for every pending change">
    <span class="select-count">{confirming === "accept" ? "Accept" : "Reject"} all {filteredRows.length}?</span>
    <button type="button" class="bar-action {confirming}" disabled={busy} onclick={() => bulk(confirming, filteredRows)}>{confirming === "accept" ? "Accept all" : "Reject all"}</button>
    <button type="button" class="bar-action" onclick={() => confirming = ""}>Cancel</button>
  </div>{/if}
  {#if feedback}<p class="panel-status" role="status" aria-live="polite">{feedback}</p>{/if}
  {#if !filteredRows.length}<div class="changes-empty" role="status">
    <p class="panel-muted">{authorFilter || fileFilter || runFilter || idsFilter.length ? "No changes match these filters." : "No pending changes."}</p>
  </div>{/if}
  <div class="changes-list" role="list" aria-label="Revision queue">
    {#each filteredRows as item (item.id)}
      {@const isOpen = active === item.id}
      <article class="change-row" class:active={isOpen} role="listitem">
        <!-- The change first and whoever wrote it after it, smaller: the new
             words are the thing being reviewed, and a username set above them
             in bold reads as though the author were. -->
        <div class="row-head">{#if selecting}<input type="checkbox" class="row-pick" checked={checked.has(item.id)} aria-label={`Tick ${authorLabel(item)}'s change`} onclick={(event) => event.stopPropagation()} onchange={() => toggleChecked(item)} />{/if}<button id={`change-${item.id}`} type="button" class="row-main" aria-current={isOpen ? "true" : undefined} onclick={() => reveal(item)}><span class="row-diff">{#if item.before}<span class="deletion">− {item.before}</span>{/if}{#if item.after}<span class="insertion">+ {item.after}</span>{/if}</span><span class="row-meta"><span class="row-author">{authorLabel(item)}</span>{#if showPath}<span class="row-context">{pathOf(item)}</span>{/if}{#if item.run}<span class="row-run">{runLabel(item.run)}</span>{/if}</span></button></div>
        <!-- Only the change being looked at offers an answer. Twenty rows of
             buttons is twenty invitations to answer something nobody has read. -->
        {#if isOpen && !selecting && canReview}<div class="row-actions"><button type="button" class="row-action reject" aria-label={`Reject change in ${pathOf(item)}`} title="Reject this change" disabled={deciding.has(item.id)} onclick={() => void decide(item, "reject")}>{deciding.has(item.id) ? "Rejecting…" : "Reject"}</button><button type="button" class="row-action accept" aria-label={`Accept change in ${pathOf(item)}`} title="Accept this change" disabled={deciding.has(item.id)} onclick={() => void decide(item, "accept")}>{deciding.has(item.id) ? "Accepting…" : "Accept"}</button></div>{/if}
      </article>
    {/each}
  </div>
  <!-- The shortcuts, said once and quietly, rather than a panel of
       documentation standing where changes could be. -->
  {#if filteredRows.length}<footer class="changes-hint">J/K next · A accept · R reject</footer>{/if}
</div>

<style>
  .changes-panel { display: flex; flex-direction: column; min-height: 0; outline: none; height: 100%; }
  /* Two lines of header, and the filter under it: about fifty pixels before
     the first change, where the settings area this replaced took several
     hundred. Both stay put while the queue scrolls beneath them. */
  .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; padding: .4rem var(--spacing) .25rem; background: var(--color-sidebar); }
  .changes-title-row { display: flex; flex-wrap: wrap; align-items: center; gap: .25rem .5rem; }
  h2 { margin: 0; flex: 1 1 auto; min-width: 0; }
  .title-controls { display: flex; align-items: center; gap: .15rem; margin-left: auto; min-width: 0; }
  /* The quiet tier. Only -400 is grey in the surface ramp -- -500 and -600
     are the theme's ink -- so these read as body text rather than as meta.
     `--panel-muted` is what every other panel means by quiet. */
  .changes-meta, .row-meta, .changes-hint { color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .changes-meta { margin-top: .1rem; }
  /* The track and thumb are `.switch` in librepaper.css, worn here and in the
     settings dialog alike; this is only where the words sit beside them. */
  .changes-panel :global(.tracking-toggle) { display: flex; align-items: center; gap: .35rem; flex: 0 0 auto; font-size: var(--panel-meta-size); white-space: nowrap; }
  .filter-bar { flex: 0 0 auto; padding: 0 var(--spacing) .3rem; background: var(--color-sidebar); border-bottom: 1px solid var(--color-divider); }
  .filter-trigger { display: flex; align-items: center; gap: .3rem; max-width: 100%; padding: .15rem .45rem; border: 1px solid var(--color-divider); border-radius: .25rem; background: none; color: inherit; font-size: var(--panel-font-size); line-height: var(--panel-line-height); text-align: left; cursor: pointer; }
  .filter-trigger:hover, .filter-trigger[data-state="open"] { background: var(--color-divider); }
  .filter-trigger :global(svg) { color: var(--panel-muted); flex: 0 0 auto; }
  .filter-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .changes-menu { flex: 0 0 auto; padding: .1rem .35rem; border: 0; border-radius: .25rem; background: none; line-height: 1; cursor: pointer; }
  .changes-menu:hover, .changes-menu[data-state="open"] { background: var(--color-divider); }
  .changes-menu-label { padding: .35rem .65rem .2rem; color: var(--panel-muted); font-size: var(--panel-meta-size); font-weight: 600; text-transform: uppercase; letter-spacing: .06em; }
  /* The contextual bar: only while a selection or a confirmation is open, and
     gone the moment it is answered or dismissed. */
  .select-bar { flex: 0 0 auto; display: flex; flex-wrap: wrap; align-items: center; gap: .3rem; padding: .35rem var(--spacing); border-bottom: 1px solid var(--color-divider); background: color-mix(in srgb, var(--color-brand) 6%, var(--color-sidebar)); font-size: var(--panel-meta-size); }
  .select-count { flex: 1 1 auto; color: var(--panel-muted); }
  .bar-action, .row-action { border: 0; background: none; padding: .2rem .3rem; border-radius: .25rem; font-size: var(--panel-meta-size); cursor: pointer; }
  .row-action { padding-inline: .4rem; }
  .bar-action:hover:not(:disabled), .row-action:hover:not(:disabled) { background: var(--color-divider); }
  .bar-action:disabled, .row-action:disabled { opacity: .4; cursor: default; }
  .bar-action.accept, .row-action.accept { color: var(--color-success-text); }
  .bar-action.reject, .row-action.reject { color: var(--color-error-text); }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; overscroll-behavior: contain; }
  /* A row is the change and a line saying whose it is. No card, no padding
     around a card, no border but the hairline between one change and the
     next -- so a dozen of them fit where three used to. */
  .change-row { position: relative; content-visibility: auto; contain-intrinsic-size: 0 3.4rem; border-bottom: 1px solid var(--color-divider); padding: .5rem var(--spacing) .5rem calc(var(--spacing) - 3px); border-left: 3px solid transparent; }
  /* One mark for the selected change, not three: a rule down its left edge. */
  .change-row.active { border-left-color: var(--color-brand); }
  .row-head { display: flex; align-items: flex-start; gap: .4rem; }
  .row-pick { flex: none; margin-top: .25rem; }
  .row-main { min-width: 0; flex: 1; text-align: left; background: none; border: 0; padding: 0; cursor: pointer; }
  .row-diff, .row-meta { display: block; }
  /* The diff quotes the source, so it wears the source's monospace at the
     panel's own size rather than a hand-set one. */
  .row-diff { font-family: var(--font-editor); font-size: var(--panel-font-size); line-height: var(--panel-line-height); overflow-wrap: anywhere; }
  .row-diff > span { display: block; }
  /* A change nobody is looking at shows its first couple of lines; the one
     being reviewed shows all of it. */
  .change-row:not(.active) .row-diff > span { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; overflow: hidden; }
  .insertion { color: var(--color-success-text); }
  .deletion { color: var(--color-error-text); text-decoration: line-through; text-decoration-color: color-mix(in srgb, currentColor 55%, transparent); }
  /* One metadata line, whatever it ends up carrying: the parts are written
     without their separators and the line puts them between. */
  .row-meta { margin-top: .15rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row-meta > span + span::before { content: " · "; }
  .row-actions { display: flex; justify-content: flex-end; gap: .5rem; margin-top: .3rem; }
  .changes-empty { padding: 1rem var(--spacing); }
  .changes-hint { flex: 0 0 auto; padding: .35rem var(--spacing); border-top: 1px solid var(--color-divider); text-align: center; }
  @media (max-width: 32rem) { .changes-header, .filter-bar, .select-bar, .change-row { padding-inline: calc(var(--spacing) * .75); } .change-row { padding-left: calc(var(--spacing) * .75 - 3px); } .row-context { max-width: 10rem; } }
</style>
