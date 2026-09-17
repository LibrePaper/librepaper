<script>
  import IconButton from "./IconButton.svelte";
  import CommentCard from "./CommentCard.svelte";
  import CommentComposer from "./CommentComposer.svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import { reviewGroups, rejectPass } from "../lib/assistant-review.js";
  import { MODES } from "../lib/annotating.js";
  import { tick } from "svelte";

  // The annotations, in document order: an annotation is about a place in the
  // text, so the column follows the page rather than the order things were
  // written. A note on a figure sorts by where that figure sits in the text.
  // Anything that could not be anchored has no place to sort by, so it goes to
  // the end, in the order it was made.
  let {
    comments = [],
    identity = "",
    commentingAs = "Anonymous",
    canModerate = false,
    canComment = true,
    // The armed mode, if any: "point", or "" for the ordinary
    // state, where a selection in the document is what starts an annotation.
    mode = "",
    // The draft being written, if any: `{ pending, verb, prefill }`. It is
    // shown as a card in the column, in the place the saved note will take.
    composing = null,
    needsLogin = false,
    signInHref = "",
    onsend,
    oncancel,
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
    emptyMessage = "Select a passage in the document, then choose Comment.",
    onhistory,
    cardIdPrefix = "comment",
    // The id of the annotation the page has singled out, if any.
    selected = "",
  } = $props();

  function place(comment) {
    return Number.isFinite(comment.start) ? comment.start : Infinity;
  }

  // Where a draft sits while it is being written: the same rule, over the
  // anchor the selection bar captured rather than over a saved comment, so
  // the composer occupies the place its card will take rather than jumping
  // there when it is sent.
  function placeDraft(pending) {
    if (!pending) return Infinity;
    return Number.isFinite(pending.position) ? pending.position : Infinity;
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

  // Commenting, highlighting and suggesting are not offered here: they act on
  // a passage, and the bar over the selection is where they belong. What is
  // left are the two annotations with nothing to select, which is why they
  // are still armed in advance. See `lib/annotating.js`.
  const shownModes = $derived(filter === "highlights" ? [] : MODES);

  // Where the draft's card goes: before the first group whose passage is
  // later in the document than the one being written about, or at the end.
  const composerKey = $derived.by(() => {
    if (!composing) return null;
    const where = placeDraft(composing.pending);
    return groups.find((group) => place(group.comments[0]) > where)?.key ?? "";
  });

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
        <!-- Not a radiogroup: neither of these is on by default, and choosing
             the one that is on puts it away again, which is a pair of toggles
             rather than a choice between two. -->
        <div class="tools flex gap-1" role="group" aria-label="Add a note">
        {#each shownModes as item (item.id)}
          <IconButton
            icon={item.icon}
            size="btn-icon-sm"
            label={item.label}
            tool={item.id}
            pressed={mode === item.id}
            disabled={!canComment}
            title={!canComment ? "Read-only access"
              : mode === item.id ? `${item.label} · on. Choose again to stop.` : item.title}
            onclick={() => ontool?.(item.id)}
          />
        {/each}
        </div>
        {#if onhistory}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={onhistory}>Show changes since…</button>
        {/if}
        {#if canComment && (resolvable.length || clearable.length || (canModerate && deletable.length))}
          <div class="bulk flex gap-1" role="group" aria-label="All comments">
            {#if resolvable.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                title="Resolve every open comment"
                onclick={resolveAll}>Resolve all</button>
            {/if}
            {#if clearable.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                title="Delete every resolved comment"
                disabled={!ondeletemany}
                onclick={() => ondeletemany?.(clearable)}>Clear resolved</button>
            {/if}
            {#if canModerate && deletable.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
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

  <!-- Keyed on the draft: the composer keeps what is being typed in its own
       state, so a second draft has to be a second component rather than the
       first one handed a new passage to be about. -->
  {#snippet composer()}
    {#key composing.id}
    <CommentComposer
      pending={composing.pending}
      verb={composing.verb}
      prefill={composing.prefill || ""}
      {identity}
      {commentingAs}
      {needsLogin}
      {signInHref}
      onsend={(written) => onsend?.(written)}
      oncancel={() => oncancel?.()}
    />
    {/key}
  {/snippet}

  <div id="{cardIdPrefix}-list" class="comments-list flex flex-col gap-3">
    {@render pending?.()}
    {#if passResult}<p class="panel-muted" role="status">{passResult}</p>{/if}
    {#each groups as group (group.key)}
      {#if composerKey === group.key}{@render composer()}{/if}
      <div class="flex flex-col gap-3" data-pass={group.pass || undefined}>
      {#if group.pass}
        <div class="pass-header">
          <strong>Writing pass · {group.comments.length} suggestions</strong>
          <p class="panel-meta">{group.pending.length} pending</p>
          {#if canModerate && group.pending.length}
            <div class="flex gap-2">
              <button class="btn btn-sm preset-outlined-surface-300-700" disabled={Boolean(rejecting)} onclick={() => void reviewNext(group)}>Review next</button>
              <button class="btn btn-sm preset-outlined-surface-300-700" disabled={Boolean(rejecting) || !onrejectconfirmed} onclick={() => void rejectGroup(group)}>{rejecting === group.pass ? "Rejecting…" : "Reject all"}</button>
            </div>
          {/if}
        </div>
      {/if}
      {#each group.comments as comment (comment)}
      <CommentCard
        {canComment}
        {comment}
        cardIdPrefix={cardIdPrefix}
        selected={String(comment.id) === String(selected)}
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
    {#if composerKey === ""}{@render composer()}{/if}
  </div>
</div>

<style>
  .comments-panel { display: flex; flex-direction: column; overflow: hidden; }
  .comments-panel > :global(*) { flex-shrink: 0; }
  .comments-list { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .pass-header { padding: calc(var(--spacing) * 2); border-left: 3px solid var(--color-primary-500); }
  .bulk { margin-left: auto; }
</style>
