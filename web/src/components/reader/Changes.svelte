<script>
  import { Menu, Switch } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";

  let {
    revisions = [], comments = [], files = [], tracking = false, markup = true, showMarkup,
    canTrack = true, canReview = true, selected = "", selectedRevision = "", filters = {},
    authors = [], sessions = [], ontracking, onmarkup, onfilter, onselect,
    onaccept, onreject, onbulk, onrevisionreveal, onrevisiondecide, onrevisionundo,
    onundo, onprevious, onnext, onreveal, onresolve, ondelete, onreply, onhistory,
    onrevisionresolve, identity = "", commentingAs = "Anonymous", canModerate = false,
    went = {}, replacements = {},
  } = $props();

  let expanded = $state("");
  let checked = $state(new Set());
  let deciding = $state(new Set());
  let localFilters = $state({ status: "pending", author: "", file: "", session: "" });
  let feedback = $state("");
  let undoBatch = $state(null);
  let filtersOpen = $state(false);

  const value = (item, ...keys) => keys.map((key) => item?.[key])
    .find((answer) => answer !== undefined && answer !== null && answer !== "") ?? "";
  const idOf = (item) => String(value(item, "id", "revisionId", "revision_id"));
  const rowId = (item) => item.__kind === "suggestion" ? `suggestion:${idOf(item)}` : idOf(item);
  const pathOf = (item) => value(item, "path", "file", "filePath", "sourcePath") || "Untitled";
  const authorOf = (item) => value(item, "author", "authorName", "creator") || "Unknown author";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const authorLabel = (item) => {
    const explicit = value(item, "authorDisplayName", "displayName", "author_name", "creatorName");
    const recorded = authorOf(item);
    const raw = explicit || (identity && recorded === identity && commentingAs ? commentingAs : recorded);
    if (uuid.test(raw)) return "Unknown editor";
    const local = String(raw).split("@")[0];
    const words = local.split(/[._-]+/).filter(Boolean);
    return words.map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join(" ") || "Unknown editor";
  };
  const authorFilterLabel = (author) => authorLabel({ author });
  const createdOf = (item) => value(item, "created_at", "createdAt", "created");
  const timeLabel = (item) => {
    const date = new Date(createdOf(item));
    if (!Number.isFinite(date.getTime())) return "";
    const seconds = Math.round((date.getTime() - Date.now()) / 1000);
    const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
    if (Math.abs(seconds) < 60) return Math.abs(seconds) < 10 ? "just now" : formatter.format(seconds, "second");
    const minutes = Math.round(seconds / 60);
    if (Math.abs(minutes) < 60) return formatter.format(minutes, "minute");
    const hours = Math.round(minutes / 60);
    if (Math.abs(hours) < 24) return formatter.format(hours, "hour");
    return formatter.format(Math.round(hours / 24), "day");
  };
  const sessionOf = (item) => value(item, "session", "sessionId", "revisionSession");
  const statusOf = (item) => value(item, "status", "outcome") || (item.resolved ? (item.outcome || "accepted") : "pending");
  const pending = (item) => statusOf(item) === "pending";
  const positionOf = (item) => { const n = Number(value(item, "position", "start_offset", "start", "from")); return Number.isFinite(n) ? n : Number.MAX_SAFE_INTEGER; };

  // session.list() is the stable file order. File identity wins over path so
  // renames do not reshuffle an existing review queue.
  const fileOrder = $derived.by(() => {
    const order = new Map();
    files.forEach((file, index) => {
      for (const key of [value(file, "id", "file_id"), file?.path]) if (key !== "" && key !== undefined && !order.has(String(key))) order.set(String(key), index);
    });
    return order;
  });
  const allRows = $derived([
    ...revisions.map((revision) => ({ ...revision, __kind: "revision" })),
    ...comments.filter((comment) => comment.motivation === "editing").map((comment) => ({ ...comment, __kind: "suggestion" })),
  ]);
  const orderOf = (item) => fileOrder.get(String(value(item, "file_id", "fileId") || pathOf(item))) ?? Number.MAX_SAFE_INTEGER;
  const orderedRows = $derived([...allRows].sort((a, b) => orderOf(a) - orderOf(b) || pathOf(a).localeCompare(pathOf(b)) || positionOf(a) - positionOf(b) || String(value(a, "created_at", "created")).localeCompare(String(value(b, "created_at", "created"))) || rowId(a).localeCompare(rowId(b))));
  const activeFilters = $derived({ ...localFilters, ...(filters || {}) });
  const filteredRows = $derived(orderedRows.filter((item) => {
    const status = activeFilters.status || "pending";
    if (status === "pending" && !pending(item)) return false;
    if (status !== "pending" && status !== "all" && statusOf(item) !== status) return false;
    return (!activeFilters.author || authorOf(item) === activeFilters.author) && (!activeFilters.file || pathOf(item) === activeFilters.file) && (!activeFilters.session || sessionOf(item) === activeFilters.session);
  }));
  const pendingRows = $derived(filteredRows.filter(pending));
  const allPending = $derived(allRows.filter(pending).length);
  const selectedRows = $derived(filteredRows.filter((item) => checked.has(rowId(item))));
  const active = $derived(filteredRows.some((item) => rowId(item) === expanded) ? expanded : filteredRows.some((item) => rowId(item) === String(selectedRevision)) ? String(selectedRevision) : filteredRows.some((item) => rowId(item) === `suggestion:${String(selected)}`) ? `suggestion:${String(selected)}` : filteredRows[0] ? rowId(filteredRows[0]) : "");
  const markupVisible = $derived(showMarkup === undefined ? markup : showMarkup);
  const derivedAuthors = $derived(authors.length ? authors : [...new Set(allRows.map(authorOf))]);
  const derivedFiles = $derived(files.length ? files.map((file) => file.path || file.id).filter(Boolean) : [...new Set(allRows.map(pathOf))]);
  const rowFor = (id) => filteredRows.find((item) => rowId(item) === id);
  const reviewAllowed = (item) => item?.__kind === "suggestion" ? canModerate : canReview;
  const activeIndex = $derived(filteredRows.findIndex((item) => rowId(item) === active));
  const showExtraFilters = $derived(derivedAuthors.length > 1 || derivedFiles.length > 1);

  $effect(() => {
    const valid = new Set(allRows.filter(pending).map(rowId));
    const next = new Set([...checked].filter((id) => valid.has(id)));
    if (next.size !== checked.size) checked = next;
  });
  $effect(() => { if (expanded && !filteredRows.some((item) => rowId(item) === expanded)) expanded = ""; });

  function blocker(item) {
    if (!item) return "";
    const ids = item.affectedRevisionIds || item.dependencies || [];
    if (item.conflict) return typeof item.conflict === "string" ? item.conflict : "Concurrent edits need attention.";
    if (item.dependency || ids.length) return typeof item.dependency === "string" ? item.dependency : "This change depends on another revision.";
    return "";
  }
  function updateFilter(key, event) { const next = { ...localFilters, [key]: event.currentTarget.value }; localFilters = next; onfilter?.(next); }
  function focusRow(id) { requestAnimationFrame(() => document.getElementById(`change-${id}`)?.focus({ preventScroll: true })); }
  function activate(item, { focus = false } = {}) {
    if (!item) return;
    expanded = rowId(item); onselect?.(item);
    if (item.__kind === "suggestion") onreveal?.(item); else onrevisionreveal?.(item);
    if (focus) focusRow(rowId(item));
  }
  function reveal(item) { activate(item); }
  function toggleChecked(item) { const id = rowId(item); const next = new Set(checked); next.has(id) ? next.delete(id) : next.add(id); checked = next; }
  function decisionPromise(item, action) {
    const callback = item.__kind === "suggestion" ? (action === "accept" ? onaccept : onreject) : onrevisiondecide;
    if (!callback) return Promise.reject(new Error("Review action is unavailable."));
    return item.__kind === "suggestion" ? callback(item) : callback(idOf(item), action);
  }
  function addBusy(id) { deciding = new Set([...deciding, id]); }
  function removeBusy(id) { deciding = new Set([...deciding].filter((current) => current !== id)); }
  function nextAfter(ids, oldIndex) { return [...ids.slice(oldIndex + 1), ...ids.slice(0, oldIndex).reverse()].map(rowFor).find((item) => item && pending(item)); }

  async function decide(item, action) {
    const id = rowId(item); const why = blocker(item);
    if (!pending(item) || !reviewAllowed(item) || deciding.has(id)) return;
    if (why) { feedback = `Cannot ${action} this change: ${why}`; activate(item); return; }
    addBusy(id); feedback = "";
    const ids = pendingRows.map(rowId); const oldIndex = ids.indexOf(id);
    try {
      const operation = decisionPromise(item, action);
      if (operation === undefined || operation === false) throw new Error("Review action is unavailable.");
      await operation;
      undoBatch = item.__kind === "revision" ? { action, ids: [id] } : null;
      const next = nextAfter(ids, oldIndex);
      if (next) activate(next, { focus: true }); else { expanded = ""; requestAnimationFrame(() => document.querySelector(".changes-panel")?.focus()); }
    } catch (error) { feedback = error?.message || `Could not ${action} this change.`; focusRow(id); }
    finally { removeBusy(id); }
  }

  async function bulk(action) {
    if (!canReview) return;
    // Capture IDs before sending anything; this is not a moving filter query.
    const captured = selectedRows.filter(pending); if (!captured.length) return;
    const excluded = captured.filter((item) => blocker(item) || !reviewAllowed(item));
    const eligible = captured.filter((item) => !blocker(item) && reviewAllowed(item));
    if (!eligible.length) { feedback = `${excluded.length} selected change${excluded.length === 1 ? "" : "s"} excluded because it needs attention.`; return; }
    const ids = eligible.map(idOf); eligible.forEach((item) => addBusy(rowId(item))); checked = new Set();
    try {
      const supplied = onbulk?.({ action, decision: action, ids: [...ids], revisions: [...eligible] });
      let report;
      if (supplied !== undefined) report = await supplied;
      else if (onbulk) throw new Error("Bulk review did not return a confirmation report.");
      else {
        const settled = await Promise.allSettled(eligible.map((item) => decisionPromise(item, action)));
        report = { succeeded: settled.flatMap((entry, index) => entry.status === "fulfilled" ? [ids[index]] : []), failed: settled.flatMap((entry, index) => entry.status === "rejected" ? [{ id: ids[index], error: entry.reason }] : []) };
      }
      if (!report) throw new Error("Bulk review did not return a confirmation report.");
      const succeeded = Array.isArray(report) ? report.filter((entry) => entry.status === "fulfilled").length : (report?.succeeded?.length ?? ids.length);
      const failed = Array.isArray(report) ? report.filter((entry) => entry.status === "rejected").length : (report?.failed?.length ?? 0);
      const omitted = excluded.length + Math.max(0, eligible.length - succeeded - failed);
      feedback = failed || omitted ? `${succeeded} ${action}ed; ${failed} failed; ${omitted} excluded or still pending.` : `${succeeded} change${succeeded === 1 ? "" : "s"} ${action}ed.`;
      const doneIds = Array.isArray(report)
        ? report.flatMap((entry, index) => entry.status === "fulfilled" ? [ids[index]] : [])
        : Array.isArray(report?.succeeded) ? report.succeeded : ids.slice(0, succeeded);
      const liveIds = doneIds.map(String).filter((doneId) => eligible.some((item) => item.__kind === "revision" && idOf(item) === doneId));
      if (liveIds.length) undoBatch = { action, ids: liveIds };
    } catch (error) { feedback = error?.message || `Could not ${action} the selected changes.`; }
    finally { eligible.forEach((item) => removeBusy(rowId(item))); }
  }
  async function undo() {
    const batch = undoBatch; if (!batch) return; undoBatch = null;
    const undoAction = onrevisionundo || onundo;
    if (!undoAction) { feedback = "Review undo is unavailable for this action."; return; }
    const settled = await Promise.allSettled(batch.ids.map((id) => {
      const operation = undoAction(id);
      return operation === undefined || operation === false
        ? Promise.reject(new Error("Review undo did not return a confirmation.")) : operation;
    }));
    const failed = settled.filter((entry) => entry.status === "rejected");
    feedback = failed.length ? `${batch.ids.length - failed.length} undone; ${failed.length} could not be undone. The affected changes remain pending.` : `Undid ${batch.action === "accept" ? "acceptance" : "rejection"}${batch.ids.length > 1 ? "s" : ""}.`;
  }
  function chooseMenuItem(value) {
    if (value === "markup") onmarkup?.(!markupVisible);
    else if (value === "undo") void undo();
  }
  function move(offset) { const index = filteredRows.findIndex((item) => rowId(item) === active); const item = filteredRows[index + offset] || filteredRows[index]; if (item) activate(item, { focus: true }); }
  function resolve(item) {
    const ids = item.affectedRevisionIds || item.dependencies || [idOf(item)];
    onrevisionreveal?.(item);
    if (onrevisionresolve) onrevisionresolve(ids);
    else feedback = "Source-based resolution is unavailable for this conflict; review the affected passages individually.";
  }
  function keydown(event) {
    if (event.defaultPrevented || event.isComposing || event.target.closest("input,select,textarea,[contenteditable=true],details,[data-scope='menu']")) return;
    const item = rowFor(active); const key = event.key.toLowerCase();
    if (event.key === "ArrowDown" || key === "j") { event.preventDefault(); onnext?.(active); move(1); }
    else if (event.key === "ArrowUp" || key === "k") { event.preventDefault(); onprevious?.(active); move(-1); }
    else if (item && key === "a" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "accept"); }
    else if (item && key === "r" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); void decide(item, "reject"); }
  }
  function shortDiff(item) {
    const before = value(item, "before", "oldText", "deleted", "exact"); const after = value(item, "after", "newText", "inserted", "proposed");
    const result = item.__kind === "suggestion" ? `Proposal: ${before || "∅"} → ${after || "∅"}` : item.kind === "delete" || item.kind === "deletion" || (before && !after) ? `− ${before}` : item.kind === "insert" || item.kind === "insertion" || (!before && after) ? `+ ${after}` : before || after ? `${before || "∅"} → ${after || "∅"}` : value(item, "summary", "title") || "Text changed";
    return result.length > 180 ? `${result.slice(0, 177)}…` : result;
  }
  function diffParts(item) {
    const before = value(item, "before", "oldText", "deleted", "exact");
    const after = value(item, "after", "newText", "inserted", "proposed");
    return { before, after };
  }
</script>

<!-- A focusable review context keeps shortcuts out of source and discussion fields. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="panel changes-panel" role="application" tabindex="0" onkeydown={keydown} aria-label="Changes review" aria-keyshortcuts="ArrowDown ArrowUp J K A R">
  <header class="changes-header">
    <!-- A real switch rather than a checkbox with a switch painted over it:
         the state is the control's own, so the name no longer has to carry
         "On" or "Off" for a screen reader to read it out. -->
    <div class="changes-title-row"><h2>Changes</h2><Switch class="tracking-toggle" checked={tracking} disabled={!canTrack}
      title={canTrack ? "Track subsequent edits in this document" : "Editing is unavailable"}
      onCheckedChange={({ checked }) => ontracking?.(checked)}>
      <Switch.Label>Track changes</Switch.Label>
      <Switch.Control class="switch"><Switch.Thumb class="switch-thumb" /></Switch.Control>
      <Switch.HiddenInput />
    </Switch></div>
    <div class="changes-meta" aria-live="polite">{allPending} pending{allRows.length !== allPending ? ` · ${allRows.length} total` : ""}{pendingRows.length !== allPending ? ` · ${pendingRows.length} shown` : ""}</div>
    <div class="filter-bar">
      <select aria-label="Change status" value={activeFilters.status} onchange={(event) => updateFilter("status", event)}><option value="pending">Pending</option><option value="accepted">Accepted</option><option value="rejected">Rejected</option><option value="all">All statuses</option></select>
      {#if showExtraFilters}<button type="button" class="btn btn-sm preset-outlined-surface-300-700" aria-expanded={filtersOpen} aria-controls="change-extra-filters" onclick={() => filtersOpen = !filtersOpen}>Filter{activeFilters.author || activeFilters.file || activeFilters.session ? " •" : ""}</button>{/if}
      <Menu onSelect={(chosen) => chooseMenuItem(chosen.value)}>
        <!-- The button is authored here rather than handed a `class`: a class
             arriving as a prop carries no scope hash, so the rules below would
             have to be global to paint at all. -->
        <Menu.Trigger>
          {#snippet element(attributes)}
            <button {...attributes} class="changes-menu" aria-label="More change options">•••</button>
          {/snippet}
        </Menu.Trigger>
        <ExplorerMenu>
          <div class="changes-menu-label">View</div>
          <Menu.Item value="markup" class="menuitem" disabled={!onmarkup}>
            <span class="w-4">{markupVisible ? "✓" : ""}</span>Markup
          </Menu.Item>
          {#if undoBatch}
            <Menu.Item value="undo" class="menuitem"><span class="w-4"></span>Undo last decision</Menu.Item>
          {/if}
          <hr class="hr my-1" />
          <div class="changes-menu-label">Keyboard shortcuts</div>
          <div class="changes-menu-hint">J/K or ↑/↓ to move<br />A to accept · R to reject</div>
        </ExplorerMenu>
      </Menu>
    </div>
    {#if filtersOpen && showExtraFilters}<div id="change-extra-filters" class="filters" aria-label="Change filters">
      {#if derivedAuthors.length > 1}<select aria-label="Filter by author" value={activeFilters.author} onchange={(event) => updateFilter("author", event)}><option value="">All authors</option>{#each derivedAuthors as author}<option value={author}>{authorFilterLabel(author)}</option>{/each}</select>{/if}
      {#if derivedFiles.length > 1}<select aria-label="Filter by file" value={activeFilters.file} onchange={(event) => updateFilter("file", event)}><option value="">All files</option>{#each derivedFiles as file}<option value={file}>{file}</option>{/each}</select>{/if}
    </div>{/if}
    {#if feedback}<p class="panel-status" role="status" aria-live="polite">{feedback}</p>{/if}
  </header>
  {#if !filteredRows.length}<p class="panel-muted changes-empty" role="status">{activeFilters.status === "pending" ? "No pending changes." : "No changes match these filters."}</p>{/if}
  <div class="changes-list" role="list" aria-label="Revision queue">
    {#each filteredRows as item (rowId(item))}
      {@const id = rowId(item)}{@const isOpen = active === id}{@const why = blocker(item)}{@const parts = diffParts(item)}
      <article class="change-row" class:active={isOpen} class:suggestion={item.__kind === "suggestion"} class:blocked={Boolean(why)} role="listitem">
        <div class="row-head"><button id={`change-${id}`} type="button" class="row-main" aria-current={isOpen ? "true" : undefined} onclick={() => reveal(item)}><span class="row-author">{authorLabel(item)}{timeLabel(item) ? ` · ${timeLabel(item)}` : ""}</span><span class="row-diff">{#if parts.before}<span class="deletion">− {parts.before}</span>{/if}{#if parts.after}<span class="insertion">+ {parts.after}</span>{/if}{#if !parts.before && !parts.after}{shortDiff(item)}{/if}</span>{#if derivedFiles.length > 1}<span class="row-context">{pathOf(item)}</span>{/if}{#if statusOf(item) !== "pending"}<span class="row-status">{statusOf(item)}</span>{/if}</button></div>
        <div class="row-footer"><span></span>{#if pending(item)}<span class="row-actions"><button type="button" class="btn btn-sm row-action reject" aria-label={`Reject change in ${pathOf(item)}`} title={why || "Reject this change"} disabled={!reviewAllowed(item) || Boolean(why) || deciding.has(id)} onclick={() => void decide(item, "reject")}>{deciding.has(id) ? "Rejecting…" : "Reject"}</button><button type="button" class="btn btn-sm row-action accept" aria-label={`Accept change in ${pathOf(item)}`} title={why || "Accept this change"} disabled={!reviewAllowed(item) || Boolean(why) || deciding.has(id)} onclick={() => void decide(item, "accept")}>{deciding.has(id) ? "Accepting…" : "Accept"}</button></span>{/if}</div>
        {#if isOpen && why}<div id={`change-detail-${id}`} class="change-detail" role="region" aria-label={`Details for change in ${pathOf(item)}`}>
          <div class="conflict" role="alert"><strong>Needs attention</strong><p>{why}</p><button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => resolve(item)}>Resolve dependency</button></div>
        </div>{#if item.replies?.length}<details class="discussion"><summary>Discussion ({item.replies.length})</summary><ul>{#each item.replies as reply (reply.id)}<li><strong>{reply.creator || "Author"}:</strong> {reply.body}</li>{/each}</ul></details>{/if}{/if}
      </article>
    {/each}
  </div>
  {#if filteredRows.length}<footer class="queue-nav"><span>{activeIndex + 1} of {filteredRows.length}</span><span><button type="button" aria-label="Previous change" title="Previous change (↑ or K)" onclick={() => { onprevious?.(active); move(-1); }} disabled={activeIndex <= 0}>↑</button><button type="button" aria-label="Next change" title="Next change (↓ or J)" onclick={() => { onnext?.(active); move(1); }} disabled={activeIndex >= filteredRows.length - 1}>↓</button></span></footer>{/if}
</div>

<style>
  .changes-panel { display: flex; flex-direction: column; min-height: 0; outline: none; height: 100%; }
  .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; padding: var(--spacing); background: var(--color-sidebar); border-bottom: 1px solid var(--color-surface-200-800); }
  .changes-title-row, .filters, .row-head { display: flex; align-items: center; gap: var(--spacing); } .changes-title-row { flex-wrap: wrap; justify-content: space-between; } h2 { margin: 0; font-size: 1rem; }
  .changes-meta, .row-context, .row-status { color: var(--color-surface-500-400); font-size: .75rem; }
  /* The track and thumb are `.switch` in librepaper.css, worn here and in the
     settings dialog alike; this is only where the words sit beside them. */
  .changes-panel :global(.tracking-toggle) { display: flex; align-items: center; gap: .45rem; font-size: .78rem; }
  /* The header of a panel that can be dragged down to fifteen rem: a row that
     insists on a width is a row that pushes the ••• button out of the column.
     So the status select takes what is left rather than asking for seven rem
     of it, and the row wraps before it overflows. What the button opens is
     portalled to the body, so the column's own clipping is not its problem. */
  .filter-bar { display: flex; flex-wrap: wrap; gap: .4rem; margin-top: .65rem; } .filter-bar select { flex: 1 1 6rem; min-width: 0; max-width: 100%; }
  .changes-menu { margin-left: auto; padding: .2rem .45rem; border: 0; border-radius: .25rem; background: none; line-height: 1; cursor: pointer; }
  .changes-menu:hover, .changes-menu[data-state="open"] { background: var(--color-surface-200-800); }
  .changes-menu-label { padding: .35rem .65rem .2rem; color: var(--color-surface-600-400); font-size: .7rem; font-weight: 700; text-transform: uppercase; letter-spacing: .05em; }
  .changes-menu-hint { padding: 0 .65rem .35rem; color: var(--color-surface-500-400); font-size: .75rem; line-height: var(--panel-line-height); }
  .filters { flex-wrap: wrap; margin-top: .5rem; } .filters select { min-width: 0; max-width: 100%; flex: 1 1 7rem; }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; overscroll-behavior: contain; } .change-row { position: relative; content-visibility: auto; contain-intrinsic-size: 0 7rem; border-bottom: 1px solid var(--color-surface-200-800); padding: .8rem var(--spacing); } .change-row.active { background: color-mix(in srgb, var(--color-primary-500) 8%, transparent); box-shadow: inset 3px 0 var(--color-primary-500); } .change-row.blocked { box-shadow: inset 3px 0 var(--color-warning-500); }
  .row-head { align-items: flex-start; gap: .4rem; } .row-main { min-width: 0; flex: 1; text-align: left; background: none; border: 0; padding: 0; cursor: pointer; } .row-author, .row-diff, .row-context, .row-status { display: block; } .row-author { font-size: .78rem; font-weight: 600; } .row-diff { margin: .45rem 0; font: .84rem/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere; } .row-diff span { display: block; } .insertion { color: var(--color-success-700-300); } .deletion { color: var(--color-error-700-300); text-decoration: line-through; text-decoration-color: color-mix(in srgb, currentColor 55%, transparent); } .row-footer { display: flex; min-height: 1.8rem; align-items: center; justify-content: space-between; } .row-actions { display: flex; gap: .25rem; } .row-action { flex: 0 0 auto; background: transparent; } .row-action.accept { color: var(--color-success-700-300); } .row-action.reject { color: var(--color-error-700-300); }
  .change-detail { margin: .6rem 0 0 1.5rem; max-height: 16rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; } .discussion { margin: .5rem 0 0 1.5rem; font-size: .8rem; } .discussion li { margin-top: .3rem; } .conflict { margin-top: .7rem; padding: .5rem; border-left: 3px solid var(--color-warning-500); } .changes-empty { padding: 1rem; }
  .queue-nav { flex: 0 0 auto; display: flex; align-items: center; justify-content: center; gap: 1.4rem; min-height: 2.5rem; border-top: 1px solid var(--color-surface-200-800); font-size: .78rem; color: var(--color-surface-500-400); } .queue-nav button { border: 0; background: transparent; padding: .35rem .55rem; font-size: 1rem; cursor: pointer; } .queue-nav button:disabled { opacity: .3; cursor: default; }
  @media (max-width: 32rem) { .changes-header, .change-row { padding-inline: calc(var(--spacing) * .75); } .row-head { gap: .35rem; } .row-action { padding-inline: .35rem; } .row-context { max-width: 10rem; } .change-detail, .discussion { margin-left: .25rem; } }
</style>
