<script>
  import IconButton from "./IconButton.svelte";
  import CommentCard from "./CommentCard.svelte";
  import CommentComposer from "./CommentComposer.svelte";
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
    identity = "",
    commentingAs = "Anonymous",
    canModerate = false,
    canComment = true,
    // The draft being written, if any: `{ id, pending }`. It is
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
    // The standing line at the top of the panel: how a comment gets made at
    // all. It is not an empty state -- someone who has read one comment still
    // has to be told where the next one comes from -- so it stays put.
    hint = "Select text in the preview window to comment.",
    onhistory,
    cardIdPrefix = "comment",
    // The id of the annotation the page has singled out, if any.
    selected = "",
    // The traversal behind `comments`, which is a prefix of the document's
    // comments rather than all of them (docs/protocol/comments-v1.md).
    // `null` for a caller that mounts this over a fixed list.
    page = null,
    onloadmore,
    onloadreplies,
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

  // What the header says. The totals are the catalogue's, never
  // `comments.length`: a reader holding fifty of four hundred has to be told
  // four hundred, or every count on the panel is a claim about the page
  // dressed up as a claim about the document.
  //
  // A filtered tab is the exception and says so. The server counts comments,
  // not comments-of-this-kind, so "12 of the 50 loaded so far" is the honest
  // wording there rather than a document-wide figure this cannot know.
  const partial = $derived(Boolean(page) && !page.complete);
  const meta = $derived.by(() => {
    if (!page) return filtered.length ? `${open} open · ${filtered.length} total` : undefined;
    if (page.loading) return "Loading…";
    if (filter === "all") {
      return partial
        ? `${page.open} open · ${page.total} total · ${comments.length} loaded`
        : `${page.open} open · ${page.total} total`;
    }
    return partial
      ? `${open} open · ${filtered.length} of the ${comments.length} loaded so far`
      : `${open} open · ${filtered.length} total`;
  });
  // Against everything loaded, not against this tab's share of it: what
  // "Load more" fetches is comments, not comments of one kind.
  const remaining = $derived(page ? Math.max(0, page.total - comments.length) : 0);

  // The bulk verbs act only on what the cards would let this caller do one at
  // a time: resolve is not a suggestion's verb (accept and reject are), and
  // delete is real only for a comment the server said so about, or an owner.
  const resolvable = $derived(filtered.filter((comment) => !comment.resolved && comment.motivation !== "editing"));
  const deletable = $derived(filtered.filter((comment) => canModerate || comment.deletable));
  const clearable = $derived(deletable.filter((comment) => comment.resolved));

  // Commenting and highlighting are not offered here: they act on a passage,
  // and the bar over the selection is where they belong. See
  // `lib/annotating.js`.

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
<div class="comments-panel">
  <PanelHeader
    title={title}
    meta={meta}
  >
    {#snippet actions()}
      <div class="flex flex-wrap items-center gap-1">
        {#if onhistory}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={onhistory}>Show changes since…</button>
        {/if}
        {#if canComment && (resolvable.length || clearable.length || (canModerate && deletable.length))}
          <div class="bulk flex gap-1" role="group" aria-label={partial ? "Loaded comments" : "All comments"}>
            {#if resolvable.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                title={partial ? "Resolve every open comment that is loaded" : "Resolve every open comment"}
                onclick={resolveAll}>{partial ? "Resolve loaded" : "Resolve all"}</button>
            {/if}
            {#if clearable.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                title={partial ? "Delete every resolved comment that is loaded" : "Delete every resolved comment"}
                disabled={!ondeletemany}
                onclick={() => ondeletemany?.(clearable)}>Clear resolved</button>
            {/if}
            {#if canModerate && deletable.length}
              <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                title={partial ? "Delete every comment that is loaded" : "Delete every comment"}
                disabled={!ondeletemany}
                onclick={() => ondeletemany?.(deletable)}>{partial ? "Delete loaded" : "Delete all"}</button>
            {/if}
          </div>
        {/if}
      </div>
    {/snippet}
    {#if !canComment}
      <p class="panel-muted">Read-only access. Ask the owner for a Comment or Edit link to participate.</p>
    {:else}
      <p class="panel-muted">{hint}</p>
    {/if}
  </PanelHeader>

  <!-- Keyed on the draft: the composer keeps what is being typed in its own
       state, so a second draft has to be a second component rather than the
       first one handed a new passage to be about. -->
  {#snippet composer()}
    {#key composing.id}
    <CommentComposer
      pending={composing.pending}
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
      {#each group.comments as comment (comment._uiKey ?? comment.id ?? comment.temp_id)}
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
        {onloadreplies}
      />
      {/each}
      </div>
    {/each}
    {#if composerKey === ""}{@render composer()}{/if}

    <!-- What lies past the cards. Four states, and they are not the same
         thing: still arriving, arrived and empty, holding a prefix, and
         holding the lot. A failed request keeps every card above it
         exactly where it is and offers the request again. -->
    {#if page}
      {#if page.error}
        <p class="page-error" role="alert">
          <span>{page.error}</span>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
            disabled={page.busy}
            onclick={() => onloadmore?.()}>Try again</button>
        </p>
      {/if}
      {#if page.loading}
        <p class="panel-muted" role="status">Loading comments…</p>
      {:else if !page.total}
        <p class="panel-muted" role="status">No comments yet.</p>
      {:else if partial}
        <button type="button" class="load-more btn btn-sm preset-outlined-surface-300-700"
          disabled={page.busy}
          onclick={() => onloadmore?.()}>
          {page.busy ? "Loading…" : `Load more (${remaining} not loaded)`}
        </button>
      {:else}
        <p class="panel-muted" role="status">All {page.total} comments loaded.</p>
      {/if}
    {/if}
  </div>
</div>

<style>
  /* The pane the collaboration panel puts this in carries the padding. */
  .comments-panel { display: flex; flex: 1 1 0; min-height: 0; flex-direction: column; overflow: hidden; }
  .comments-panel > :global(*) { flex-shrink: 0; }
  .comments-list { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .load-more { align-self: center; margin: calc(var(--spacing) * 2) 0; }
  .page-error { padding: calc(var(--spacing) * 2); display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); align-items: flex-start; }
  .pass-header { padding: calc(var(--spacing) * 2); border-left: 3px solid var(--color-primary-500); }
  .bulk { margin-left: auto; }
</style>
