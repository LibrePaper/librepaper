<script>
  import CommentCard from "../CommentCard.svelte";

  let {
    revisions = [], comments = [], tracking = false, markup = true, showMarkup,
    canEdit = true, canTrack = canEdit, canReview = true, selected = "", selectedRevision = "", filters = {},
    authors = [], files = [], sessions = [],
    ontracking, onmarkup, onfilter, onselect, onaccept, onreject, onbulk,
    onrevisionreveal, onrevisiondecide, onrevisionundo, onrevisionresolve,
    onundo, onprevious, onnext, onreveal, onresolve, ondelete, ondeletemany,
    onreply, onrejectconfirmed, onhistory,
    identity = "", commentingAs = "Anonymous", canModerate = false,
    went = {}, replacements = {},
  } = $props();

  let expanded = $state("");
  let localFilters = $state({ status: "pending", author: "", file: "", session: "" });
  let checked = $state(new Set());
  let status = $state("");
  let undoLabel = $state("");
  let undoId = $state("");
  let deciding = $state(new Set());

  const value = (item, ...keys) => keys.map((key) => item?.[key]).find((v) => v !== undefined && v !== null && v !== "") ?? "";
  const idOf = (item) => String(value(item, "id", "revisionId", "revision_id"));
  const stateOf = (item) => value(item, "status", "outcome") || (item.resolved ? (item.outcome || "accepted") : "pending");
  const pending = (item) => stateOf(item) === "pending" || !["accepted", "rejected"].includes(stateOf(item));
  const pathOf = (item) => value(item, "file", "path", "filePath", "sourcePath") || "Untitled";
  const authorOf = (item) => value(item, "author", "authorName", "creator") || "Unknown author";
  const sessionOf = (item) => value(item, "session", "sessionId", "revisionSession") || "";
  const positionOf = (item) => { const position = Number(value(item, "position", "start_offset", "start", "from")); return Number.isFinite(position) ? position : Number.MAX_SAFE_INTEGER; };
  const allRows = $derived([
    ...revisions.map((revision) => ({ ...revision, __kind: "revision", __id: idOf(revision) })),
    ...comments.filter((comment) => comment.motivation === "editing").map((comment) => ({ ...comment, __kind: "legacy", __id: `legacy:${comment.id}` })),
  ]);
  const filteredRows = $derived(allRows.filter((item) => {
    const activeFilters = { ...localFilters, ...filters }; const sf = activeFilters.status || "pending"; const state = stateOf(item);
    if (sf === "pending" && !pending(item)) return false;
    if (sf !== "pending" && sf !== "all" && state !== sf) return false;
    return (!activeFilters.author || authorOf(item) === activeFilters.author) && (!activeFilters.file || pathOf(item) === activeFilters.file) && (!activeFilters.session || sessionOf(item) === activeFilters.session);
  }).sort((a, b) => pathOf(a).localeCompare(pathOf(b)) || positionOf(a) - positionOf(b) || a.__id.localeCompare(b.__id)));
  const pendingRows = $derived(filteredRows.filter(pending));
  const selectedRows = $derived(filteredRows.filter((item) => checked.has(item.__id)));
  const active = $derived(expanded || selectedRevision || selected || filteredRows[0]?.__id || "");
  const markupVisible = $derived(showMarkup === undefined ? markup : showMarkup);
  const reviewAllowed = $derived(canReview && canModerate);

  $effect(() => { if (expanded && !filteredRows.some((item) => item.__id === expanded)) expanded = ""; });
  function updateFilter(key, event) { localFilters = { ...localFilters, [key]: event.currentTarget.value }; onfilter?.({ ...localFilters, [key]: event.currentTarget.value }); }
  function reveal(item) { expanded = item.__id; onselect?.(item); item.__kind === "legacy" ? onreveal?.(item) : onrevisionreveal?.(item); }
  function toggleChecked(id) { const next = new Set(checked); next.has(id) ? next.delete(id) : next.add(id); checked = next; }
  function decide(item, decision) {
    if (!reviewAllowed || !pending(item) || item.deciding || deciding.has(item.__id)) return;
    const next = new Set(deciding); next.add(item.__id); deciding = next;
    const orderedIds = pendingRows.map((row) => row.__id); const oldIndex = orderedIds.indexOf(item.__id);
    const operation = item.__kind === "legacy" ? (decision === "accept" ? onaccept?.(item) : onreject?.(item)) : onrevisiondecide?.(idOf(item), decision);
    Promise.resolve(operation)
      .then(() => { undoLabel = `Undo ${decision}`; undoId = idOf(item); advance(orderedIds, oldIndex); })
      .catch((error) => { status = error?.message || `Could not ${decision} this change.`; })
      .finally(() => { const done = new Set(deciding); done.delete(item.__id); deciding = done; });
  }
  function advance(orderedIds, oldIndex) { const nextId = orderedIds[oldIndex + 1] || orderedIds[oldIndex - 1]; const next = pendingRows.find((item) => item.__id === nextId); if (next) { expanded = next.__id; next.__kind === "legacy" ? onreveal?.(next) : onrevisionreveal?.(next); requestAnimationFrame(() => document.getElementById(`change-${next.__id}`)?.focus()); } else { expanded = ""; document.querySelector(".changes-panel")?.focus(); } }
  function move(offset) { const index = filteredRows.findIndex((item) => item.__id === active); const next = filteredRows[index + offset] || filteredRows[index]; if (next) { expanded = next.__id; onrevisionreveal?.(next); requestAnimationFrame(() => document.getElementById(`change-${next.__id}`)?.focus()); } }
  function bulk(decision) {
    const rows = selectedRows.filter(pending); const ids = rows.map((item) => item.__id);
    if (!ids.length) return;
    if (!reviewAllowed) return;
    const safeRows = rows.filter((item) => !item.conflict && !item.dependency && !item.dependencies?.length);
    const excluded = rows.length - safeRows.length;
    const result = onbulk?.({ decision, ids: safeRows.map(idOf), revisions: safeRows }) || Promise.allSettled(safeRows.map((item) => onrevisiondecide?.(idOf(item), decision)));
    checked = new Set();
    if (result?.then) result.then((report) => { const failed = Array.isArray(report) ? report.filter((entry) => entry.status === "rejected") : (report?.failed || []); const succeeded = Array.isArray(report) ? report.filter((entry) => entry.status === "fulfilled").length : (report?.succeeded?.length || 0); if (failed.length || excluded) status = `${succeeded} completed; ${failed.length + excluded} excluded or still pending.`; undoLabel = `Undo ${decision}`; }).catch((error) => { status = error?.message || "Some changes could not be reviewed."; });
  }
  function keydown(event) {
    if (event.target.closest("input,select,textarea,[contenteditable=true]")) return;
    const item = filteredRows.find((row) => row.__id === active);
    if (event.key === "ArrowDown") { event.preventDefault(); onnext?.(active); move(1); }
    else if (event.key === "ArrowUp") { event.preventDefault(); onprevious?.(active); move(-1); }
    else if (item && event.key.toLowerCase() === "a" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); decide(item, "accept"); }
    else if (item && event.key.toLowerCase() === "r" && !event.metaKey && !event.ctrlKey) { event.preventDefault(); decide(item, "reject"); }
  }
  function shortDiff(item) {
    const oldText = value(item, "oldText", "before", "deleted", "exact"); const newText = value(item, "newText", "after", "inserted", "proposed");
    const result = item.kind === "deletion" || (oldText && !newText) ? `− ${oldText}` : item.kind === "insertion" || (!oldText && newText) ? `+ ${newText}` : oldText || newText ? `${oldText || "∅"} → ${newText || "∅"}` : value(item, "summary", "title") || "Text changed";
    return result.length > 180 ? `${result.slice(0, 177)}…` : result;
  }
  const detail = (item) => {
    const explicit = value(item, "detail", "fullText", "context", "description");
    if (explicit) return explicit;
    const before = value(item, "oldText", "before", "deleted", "exact");
    const after = value(item, "newText", "after", "inserted", "proposed");
    if (before || after) return `Before:\n${before || "∅"}\n\nAfter:\n${after || "∅"}`;
    return "Text changed";
  };
</script>

<!-- The pane is an intentionally focusable keyboard review surface. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="panel changes-panel" role="application" tabindex="0" onkeydown={keydown} aria-label="Changes review">
  <header class="changes-header">
    <div class="changes-title-row"><h2>Changes</h2><button type="button" class="btn btn-sm {tracking ? 'preset-filled-primary-500' : 'preset-tonal-surface'}" aria-label={`Track changes: ${tracking ? "On" : "Off"}`} aria-pressed={tracking} disabled={!canTrack} title={canTrack ? "Track subsequent edits in this document" : "Editing is unavailable"} onclick={() => ontracking?.(!tracking)}>Track changes: {tracking ? "On" : "Off"}</button></div>
    <div class="changes-meta" aria-live="polite">{pendingRows.length} pending{allRows.length !== pendingRows.length ? ` · ${allRows.length} total` : ""}</div>
    <label class="markup-toggle"><input type="checkbox" checked={markupVisible} disabled={!onmarkup} onchange={(event) => onmarkup?.(event.currentTarget.checked)} /> Show markup</label>
    <div class="filters" aria-label="Change filters">
      <select aria-label="Change status" value={localFilters.status} onchange={(event) => updateFilter("status", event)}><option value="pending">Pending</option><option value="accepted">Accepted</option><option value="rejected">Rejected</option><option value="all">All statuses</option></select>
      <select aria-label="Filter by author" value={localFilters.author} onchange={(event) => updateFilter("author", event)}><option value="">All authors</option>{#each (authors.length ? authors : [...new Set(allRows.map(authorOf))]) as author}<option value={author}>{author}</option>{/each}</select>
      <select aria-label="Filter by file" value={localFilters.file} onchange={(event) => updateFilter("file", event)}><option value="">All files</option>{#each (files.length ? files : [...new Set(allRows.map(pathOf))]) as file}<option value={file}>{file}</option>{/each}</select>
      <select aria-label="Filter by revision session" value={localFilters.session} onchange={(event) => updateFilter("session", event)}><option value="">All sessions</option>{#each (sessions.length ? sessions : [...new Set(allRows.map(sessionOf).filter(Boolean))]) as session}<option value={session}>{session}</option>{/each}</select>
    </div>
    <div class="review-controls" role="group" aria-label="Review navigation"><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => { onprevious?.(active); move(-1); }} disabled={!filteredRows.length}>Previous</button><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => { onnext?.(active); move(1); }} disabled={!filteredRows.length}>Next</button>{#if filteredRows.find((row) => row.__id === active)}{@const activeRow = filteredRows.find((row) => row.__id === active)}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => decide(activeRow, "accept")} disabled={!reviewAllowed || !pending(activeRow)}>Accept and next</button><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => decide(activeRow, "reject")} disabled={!reviewAllowed || !pending(activeRow)}>Reject and next</button>{/if}{#if selectedRows.length}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => bulk("accept")}>Accept {selectedRows.length} selected</button><button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => bulk("reject")}>Reject {selectedRows.length} selected</button>{/if}{#if undoLabel}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={async () => { try { await (onrevisionundo?.(undoId) ?? onundo?.(undoId)); undoLabel = ""; } catch (error) { status = error?.message || "Undo could not be completed."; } }}>{undoLabel}</button>{/if}</div>
    {#if status}<p class="panel-status" role="alert">{status}</p>{/if}
  </header>
  {#if !filteredRows.length}<p class="panel-muted changes-empty">{localFilters.status === "pending" ? "No pending changes." : "No changes match these filters."}</p>{/if}
  <div class="changes-list" role="list">
    {#each filteredRows as item (item.__id)}
      {@const isOpen = active === item.__id}
      <article class="change-row" class:active={isOpen} class:legacy={item.__kind === "legacy"} role="listitem">
        <div class="row-head"><input type="checkbox" aria-label={`Select change in ${pathOf(item)}`} checked={checked.has(item.__id)} onchange={() => toggleChecked(item.__id)} disabled={!pending(item)} /><button id={`change-${item.__id}`} type="button" class="row-main" aria-expanded={isOpen} onclick={() => reveal(item)}><span class="row-kind">{item.__kind === "legacy" ? "Legacy proposal" : value(item, "kind", "type") || "Change"}</span><span class="row-diff">{shortDiff(item)}</span><span class="row-context">{pathOf(item)} · {authorOf(item)}{sessionOf(item) ? ` · ${sessionOf(item)}` : ""}</span></button>{#if pending(item)}<button type="button" class="btn btn-sm row-action" aria-label={`Accept change in ${pathOf(item)}`} disabled={!reviewAllowed || Boolean(item.deciding) || deciding.has(item.__id)} onclick={() => decide(item, "accept")}>Accept</button><button type="button" class="btn btn-sm row-action" aria-label={`Reject change in ${pathOf(item)}`} disabled={!reviewAllowed || Boolean(item.deciding) || deciding.has(item.__id)} onclick={() => decide(item, "reject")}>Reject</button>{/if}</div>
        {#if isOpen}{#if item.__kind === "legacy"}<CommentCard comment={item} {identity} {commentingAs} {canModerate} {went} replacement={replacements[item.id] ?? null} cardIdPrefix="changes-legacy" selected={String(item.id) === String(selected)} {onreveal} {onresolve} {ondelete} {onreply} {onaccept} {onreject} />{:else}<div class="change-detail"><p>{detail(item)}</p>{#if value(item, "contextBefore", "beforeContext")}<p class="panel-muted">{value(item, "contextBefore", "beforeContext")} <mark>{value(item, "newText", "after", "inserted", "proposed") || value(item, "oldText", "before", "deleted", "exact")}</mark> {value(item, "contextAfter", "afterContext")}</p>{/if}{#if item.conflict || item.dependency || item.dependencies?.length}<div class="conflict" role="alert"><strong>Needs attention</strong><p>{typeof (item.conflict || item.dependency) === "string" ? (item.conflict || item.dependency) : "This change depends on another revision."}</p>{#if (item.affectedRevisionIds || item.dependencies)?.length}<p class="panel-muted">Affected revisions: {(item.affectedRevisionIds || item.dependencies).join(", ")}</p>{/if}<button type="button" class="btn btn-sm preset-tonal-surface" onclick={() => { onrevisionreveal?.(item); onrevisionresolve?.((item.affectedRevisionIds || item.dependencies || [idOf(item)])); }}>Resolve dependency</button></div>{/if}</div>{/if}{#if item.replies?.length}<details class="discussion"><summary>Discussion ({item.replies.length})</summary><ul>{#each item.replies as reply (reply.id)}<li><strong>{reply.creator || "Author"}:</strong> {reply.body}</li>{/each}</ul></details>{/if}{/if}
      </article>
    {/each}
  </div>
</div>

<style>
  .changes-panel { display: flex; flex-direction: column; min-height: 0; outline: none; height: 100%; } .changes-header { position: sticky; top: 0; z-index: 2; flex: 0 0 auto; padding: var(--spacing); background: var(--color-surface-50-950); border-bottom: 1px solid var(--color-surface-200-800); }
  .changes-title-row, .review-controls, .filters, .row-head { display: flex; align-items: center; gap: var(--spacing); } .changes-title-row { justify-content: space-between; } h2 { margin: 0; font-size: 1rem; }
  .changes-meta, .row-context, .row-kind { color: var(--color-surface-500-400); font-size: .75rem; } .markup-toggle { display: block; margin: .4rem 0; font-size: .8rem; } .filters { flex-wrap: wrap; } .filters select { min-width: 0; max-width: 100%; flex: 1 1 7rem; } .review-controls { flex-wrap: wrap; margin-top: .5rem; }
  .changes-list { flex: 1 1 auto; min-height: 0; overflow: auto; padding: var(--spacing); } .change-row { border: 1px solid var(--color-surface-200-800); border-radius: .35rem; margin-bottom: .5rem; padding: .5rem; } .change-row.active { border-color: var(--color-primary-500); box-shadow: 0 0 0 1px var(--color-primary-500); }
  .row-head { align-items: flex-start; } .row-main { min-width: 0; flex: 1; text-align: left; background: none; border: 0; padding: 0; cursor: pointer; } .row-kind, .row-diff, .row-context { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; } .row-diff { margin: .15rem 0; font-size: .85rem; } .row-action { flex: 0 0 auto; }
  .change-detail { margin: .6rem 0 0 1.5rem; max-height: 16rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; } .discussion { margin: .5rem 0 0 1.5rem; font-size: .8rem; } .discussion li { margin-top: .3rem; } .conflict { margin-top: .7rem; padding: .5rem; border-left: 3px solid var(--color-warning-500); } .changes-empty { padding: 1rem; } mark { padding: 0 .15rem; }
  @media (max-width: 32rem) { .row-action { padding-inline: .35rem; } .row-context { max-width: 10rem; } }
</style>
