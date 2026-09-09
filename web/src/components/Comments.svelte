<script>
  import IconButton from "./IconButton.svelte";
  import CommentCard from "./CommentCard.svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import { reviewGroups, rejectPass } from "../lib/assistant-review.js";
  import { tick } from "svelte";

  // The annotations, in document order: an annotation is about a place in the
  // text, so the column follows the page rather than the order things were
  // written. A note on a figure sorts by where that figure sits in the text.
  // Anything that could not be anchored has no place to sort by, so it goes to
  // the end, in the order it was made.
  let {
    comments = [],
    figureAt = [],
    identity = "",
    commentingAs = "Anonymous",
    canModerate = false,
    canComment = true,
    tool = "commenting",
    hasFigures = false,
    // Where each orphaned passage went, by comment id. Passed through rather
    // than looked up here: the card is what says it, and the page is what
    // knows it.
    went = {},
    // Inserted side of a historical word diff, keyed by comment id. The page
    // owns fetching and anchoring this data; cards only present it.
    replacements = {},
    ontool,
    onreveal,
    onresolve,
    ondelete,
    // Several at once, behind one confirmation: the page owns that dialog.
    ondeletemany,
    onreply,
    onaccept,
    onreject,
    onrejectconfirmed,
    pending,
    // Optional view filter used by the collaboration tabs. Highlight notes
    // remain in both views when they carry a discussion; suggestion review
    // uses the same card and action plumbing with a narrower list.
    filter = "all",
    title = "Comments",
    emptyMessage = "Highlight text in the document, then choose “Comment”.",
    onhistory,
    cardIdPrefix = "comment",
    tools = null,
  } = $props();

  function place(comment) {
    if (comment.region) {
      const at = figureAt[comment.region.image_index];
      if (Number.isFinite(at)) return at;
    }
    return Number.isFinite(comment.start) ? comment.start : Infinity;
  }

  const filtered = $derived(comments.filter((comment) => {
    if (filter === "highlights") return comment.motivation === "highlighting";
    if (filter === "suggestions") return comment.motivation === "editing";
    if (filter === "comments") return comment.motivation !== "editing" && (comment.motivation !== "highlighting" || comment.body || comment.replies?.length);
    return true;
  }));
  const shown = $derived([...filtered].sort((a, b) => place(a) - place(b) || a.seq - b.seq));
  const groups = $derived(reviewGroups(shown));
  let rejecting = $state("");
  let passResult = $state("");

  async function rejectGroup(group) {
    if (!canModerate || rejecting || !onrejectconfirmed) return;
    rejecting = group.pass;
    passResult = "";
    try {
      const results = await rejectPass([...group.pending], onrejectconfirmed);
      const failed = results.filter((result) => !result.rejected);
      passResult = `${results.length - failed.length} rejected${failed.length ? `; ${failed.length} could not be rejected: ${failed[0].error}` : "."}`;
    } finally { rejecting = ""; }
  }

  async function reviewNext(group) {
    const next = group.pending.find((comment) => !comment.deciding);
    if (!next) return;
    await tick();
    const card = document.getElementById(`${cardIdPrefix}-${next.id}`);
    card?.scrollIntoView({ block: "nearest" });
    card?.querySelector("button")?.focus({ preventScroll: true });
  }

  const open = $derived(filtered.filter((comment) => !comment.resolved).length);

  // The bulk verbs act only on what the cards would let this caller do one at
  // a time: resolve is not a suggestion's verb (accept and reject are), and
  // delete is real only for a comment the server said so about, or an owner.
  const resolvable = $derived(filtered.filter((comment) => !comment.resolved && comment.motivation !== "editing"));
  const deletable = $derived(filtered.filter((comment) => canModerate || comment.deletable));
  const clearable = $derived(deletable.filter((comment) => comment.resolved));

  const TOOLS = [
    { id: "commenting", icon: "comment", label: "Comment", title: "Comment on the selected passage" },
    { id: "highlighting", icon: "highlight", label: "Highlight", title: "Highlight, with no comment" },
    { id: "point", icon: "comment", label: "Point comment", title: "Comment at a point in the document" },
    { id: "editing", icon: "pencil", label: "Suggest", title: "Suggest a replacement for the selected passage" },
    { id: "region", icon: "box", label: "Box", title: "Drag a box on a figure" },
  ];
  const TOOL_VIEWS = {
    comments: ["commenting", "point", "region"],
    highlights: ["highlighting"],
    suggestions: ["editing"],
  };
  const shownTools = $derived(tools ?? TOOLS.filter((item) => !TOOL_VIEWS[filter] || TOOL_VIEWS[filter].includes(item.id)));

  function resolveAll() {
    for (const comment of resolvable) onresolve?.(comment);
  }
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="panel comments-panel">
  <PanelHeader
    title={title}
    meta={filtered.length ? `${open} open · ${filtered.length} total` : undefined}
  >
    {#snippet actions()}
      <div class="flex flex-wrap items-center gap-1">
        <div class="tools flex gap-1" role="radiogroup" aria-label="Annotation tool">
        {#each shownTools as item}
          {#if filter === "suggestions"}
          <button type="button" class="btn btn-sm preset-tonal-primary" disabled={!canComment}
            aria-pressed={tool === item.id} onclick={() => ontool?.(item.id)}>{item.label}</button>
          {:else}
          <IconButton
            icon={item.icon}
            size="btn-icon-sm"
            label={item.label}
            tool={item.id}
            pressed={tool === item.id}
            disabled={!canComment || (item.id === "region" && !hasFigures)}
            title={!canComment ? "Read-only access" : item.id === "region" && !hasFigures
              ? "This document has no figures to draw on"
              : item.title}
            onclick={() => ontool?.(item.id)}
          />
          {/if}
        {/each}
        </div>
        {#if onhistory}
          <button type="button" class="btn btn-sm preset-tonal-surface" onclick={onhistory}>Show changes since…</button>
        {/if}
        {#if canComment && (resolvable.length || clearable.length || (canModerate && deletable.length))}
          <div class="bulk flex gap-1" role="group" aria-label="All comments">
            {#if resolvable.length}
              <button type="button" class="btn btn-sm preset-tonal-surface"
                title="Resolve every open comment"
                onclick={resolveAll}>Resolve all</button>
            {/if}
            {#if clearable.length}
              <button type="button" class="btn btn-sm preset-tonal-surface"
                title="Delete every resolved comment"
                disabled={!ondeletemany}
                onclick={() => ondeletemany?.(clearable)}>Clear resolved</button>
            {/if}
            {#if canModerate && deletable.length}
              <button type="button" class="btn btn-sm preset-tonal-surface"
                title="Delete every comment"
                disabled={!ondeletemany}
                onclick={() => ondeletemany?.(deletable)}>Delete all</button>
            {/if}
          </div>
        {/if}
      </div>
    {/snippet}
    {#if !canComment}
      <p class="panel-muted">Read-only access. Ask the owner for a Comment or Edit link to participate.</p>
    {:else if filtered.length === 0}
      <p class="panel-muted">
        {emptyMessage}
      </p>
    {/if}
  </PanelHeader>

  <div id="{cardIdPrefix}-list" class="comments-list flex flex-col gap-3">
    {@render pending?.()}
    {#if passResult}<p class="panel-muted" role="status">{passResult}</p>{/if}
    {#each groups as group (group.key)}
      <div class="flex flex-col gap-3" data-pass={group.pass || undefined}>
      {#if group.pass}
        <div class="pass-header">
          <strong>Writing pass · {group.comments.length} suggestions</strong>
          <p class="panel-meta">{group.pending.length} pending</p>
          {#if canModerate && group.pending.length}
            <div class="flex gap-2">
              <button class="btn btn-sm preset-tonal-surface" disabled={Boolean(rejecting)} onclick={() => void reviewNext(group)}>Review next</button>
              <button class="btn btn-sm preset-tonal-surface" disabled={Boolean(rejecting) || !onrejectconfirmed} onclick={() => void rejectGroup(group)}>{rejecting === group.pass ? "Rejecting…" : "Reject all"}</button>
            </div>
          {/if}
        </div>
      {/if}
      {#each group.comments as comment (comment)}
      <CommentCard
        {canComment}
        {comment}
        cardIdPrefix={cardIdPrefix}
        {identity}
        {commentingAs}
        {canModerate}
        went={went[comment.id] || null}
        replacement={replacements[comment.id] ?? null}
        {onreveal}
        {onresolve}
        {ondelete}
        {onreply}
        {onaccept}
        {onreject}
      />
      {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .comments-panel { display: flex; flex-direction: column; overflow: hidden; }
  .comments-panel > :global(*) { flex-shrink: 0; }
  .comments-list { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .pass-header { padding: calc(var(--spacing) * 2); border-left: 3px solid var(--color-primary-500); }
  .bulk { margin-left: auto; }
</style>
