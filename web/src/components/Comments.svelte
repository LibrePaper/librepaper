<script>
  import { m } from "../lib/paraglide/messages.js";
  import IconButton from "./IconButton.svelte";
  import CommentCard from "./CommentCard.svelte";
  import CommentComposer from "./CommentComposer.svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import MenuIconButton from "./MenuIconButton.svelte";
  import { aboutWholeDocument } from "../lib/anchor.js";

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
    onsend = undefined,
    oncancel = undefined,
    onreveal = undefined,
    onresolve = undefined,
    ondelete = undefined,
    // Several at once, behind one confirmation: the page owns that dialog.
    ondeletemany = undefined,
    onreply = undefined,
    onedit = undefined,
    onagent = undefined,
    pending = undefined,
    // Optional view filter used by the collaboration tabs. Highlight notes
    // remain in both views when they carry a discussion.
    filter = "all",
    title = m.common_comments(),
    // The standing line at the top of the panel: how a comment gets made at
    // all. It is not an empty state -- someone who has read one comment still
    // has to be told where the next one comes from -- so it stays put.
    hint = m.comments_hint(),
    // What an empty tab says: there is nothing of this tab's kind yet.
    empty = m.comments_empty(),
    onhistory = undefined,
    // Starts a comment on the document as a whole. Shown only where commenting
    // is allowed, and only by the caller that owns the draft.
    onwholedocument = undefined,
    cardIdPrefix = "comment",
    // The id of the annotation the page has singled out, if any.
    selected = "",
    // The traversal behind `comments`, which is a prefix of the document's
    // comments rather than all of them (docs/dev/protocol/comments-v1.md).
    // `null` for a caller that mounts this over a fixed list.
    page = null,
    onloadmore = undefined,
    onloadreplies = undefined,
    // Editors only: see CommentCard.
    onattach = undefined,
    attachable = false,
  } = $props();

  // A remark about the whole document has no passage, so it sorts first: it
  // is about the top of the document, which is where every passage starts.
  function place(comment) {
    if (aboutWholeDocument(comment)) return -1;
    return Number.isFinite(comment.start) ? comment.start : Infinity;
  }

  // Where a draft sits while it is being written: the same rule, over the
  // anchor the selection bar captured rather than over a saved comment, so
  // the composer occupies the place its card will take rather than jumping
  // there when it is sent.
  function placeDraft(pending) {
    if (!pending) return Infinity;
    if (pending.whole) return -1;
    return Number.isFinite(pending.position) ? pending.position : Infinity;
  }

  const filtered = $derived(comments.filter((comment) => {
    if (filter === "highlights") return comment.motivation === "highlighting";
    if (filter === "comments") return comment.motivation !== "highlighting" || comment.body || comment.replies?.length;
    return true;
  }));
  // Which of them the list shows, by whether they are settled: "open",
  // "resolved" or "all". The panel starts on what is unresolved, because what
  // is settled stays out of the way until it is asked for. This is the
  // reader's own view of the list and is kept nowhere else: the counts in the
  // bar and the bulk verbs go on speaking for the whole tab.
  let status = $state("open");
  const STATUSES = $derived([
    { value: "open", label: m.comments_filter_unresolved() },
    { value: "resolved", label: m.commentcard_resolved() },
    { value: "all", label: m.history_all_short() },
  ]);
  const statusLabel = $derived(STATUSES.find((one) => one.value === status)?.label ?? "");
  const cardKey = (comment) => comment._uiKey ?? comment.id ?? comment.temp_id;
  // Cards holding words that have not been sent: an open reply with something
  // in it, or an edit. A card says so through `onhold`.
  let holding = $state([]);
  function hold(key, on) {
    const held = holding.includes(key);
    if (on && !held) holding = [...holding, key];
    else if (!on && held) holding = holding.filter((one) => one !== key);
  }
  // Two kinds of card are listed whatever the filter says. The one the page
  // has singled out: a click on a resolved comment's passage has to be able to
  // open its card. And one holding unsent words: a thread that somebody else
  // resolves, or a change of filter, must not take what was being typed.
  const visible = $derived(filtered.filter((comment) =>
    status === "all"
      || (status === "resolved" ? Boolean(comment.resolved) : !comment.resolved)
      || String(comment.id) === String(selected)
      || holding.includes(cardKey(comment))));
  const shown = $derived([...visible].sort((a, b) => place(a) - place(b) || a.seq - b.seq));
  // A new comment is unresolved, so a draft begun while only the resolved
  // ones are listed would vanish the moment it was sent.
  $effect(() => {
    if (composing && status === "resolved") status = "open";
  });

  // The words of the draft, kept here rather than in the composer. The
  // composer's slot follows the cards around it, so a card that leaves the
  // list mid-sentence (resolved, filtered out, deleted) moves the composer to
  // another slot, which is another component: it would start again empty.
  let draftBody = $state("");
  let draftId = null;
  $effect.pre(() => {
    const id = composing?.id ?? null;
    if (id === draftId) return;
    draftId = id;
    draftBody = "";
  });

  // A card that leaves the list takes the focused element with it: resolved
  // under Unresolved, reopened under Resolved. The keyboard goes to the card
  // that took its place, or to the filter when none is left, rather than
  // being dropped on nothing.
  let list = $state(null);
  let leaving = null;
  $effect.pre(() => {
    shown;
    const card = document.activeElement?.closest?.("article");
    leaving = card && list?.contains(card)
      ? { id: card.id, next: card.nextElementSibling?.id || "", previous: card.previousElementSibling?.id || "" }
      : null;
  });
  $effect(() => {
    shown;
    if (!leaving || document.getElementById(leaving.id)) return;
    const from = leaving;
    leaving = null;
    const neighbour = (from.next && document.getElementById(from.next))
      || (from.previous && document.getElementById(from.previous))
      || null;
    const target = neighbour?.querySelector("button, textarea, a[href]")
      ?? list?.closest(".comments-panel")?.querySelector(".filter-trigger");
    target?.focus({ preventScroll: true });
  });

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
    if (!page) return filtered.length ? m.comments_meta_total({ open, total: filtered.length }) : undefined;
    // Still arriving, or arrived and empty: the list says so itself, and a
    // count over "No comments yet." says the same thing twice.
    if (page.loading || !page.total) return undefined;
    if (filter === "all") {
      return partial
        ? m.comments_meta_loaded({ open: page.open, total: page.total, loaded: comments.length })
        : m.comments_meta_total({ open: page.open, total: page.total });
    }
    if (partial) return m.comments_meta_filtered({ open, filtered: filtered.length, loaded: comments.length });
    return filtered.length ? m.comments_meta_total({ open, total: filtered.length }) : undefined;
  });
  // Against everything loaded, not against this tab's share of it: what
  // "Load more" fetches is comments, not comments of one kind.
  const remaining = $derived(page ? Math.max(0, page.total - comments.length) : 0);

  // The bulk verbs act only on what the cards would let this caller do one at
  // a time: delete is real only for a comment the server said so about, or an
  // owner.
  const resolvable = $derived(filtered.filter((comment) => !comment.resolved && (canModerate || comment.deletable)));
  const deletable = $derived(filtered.filter((comment) => canModerate || comment.deletable));
  const clearable = $derived(deletable.filter((comment) => comment.resolved));

  // Whether there is any bulk verb to offer this caller. A reader who may not
  // comment has none, and gets no menu.
  const bulkOffered = $derived(canComment && Boolean(resolvable.length || clearable.length));
  // Whether the bar over the list has anything in it.
  const headed = $derived(Boolean(meta) || Boolean(onhistory) || bulkOffered || filtered.length > 0);

  // Commenting and highlighting are not offered here: they act on a passage,
  // and the bar over the selection is where they belong. See
  // `lib/annotating.js`.

  // Where the draft's card goes: before the first group whose passage is
  // later in the document than the one being written about, or at the end.
  const composerKey = $derived.by(() => {
    if (!composing) return null;
    const where = placeDraft(composing.pending);
    const next = shown.find((comment) => place(comment) > where);
    return next ? cardKey(next) : "";
  });

  function resolveAll() {
    for (const comment of resolvable) onresolve?.(comment);
  }

  // The bulk verbs, by the value of the menu item that asks for them. The
  // deletion hands the page one list, so it can confirm once. There is no
  // "Delete all": a document's whole discussion is not one click from gone.
  function bulk(verb) {
    if (verb === "resolve") resolveAll();
    else if (verb === "clear") ondeletemany?.(clearable);
  }
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="comments-panel">
  <!-- The selected rail icon and the open tab name the panel, so its heading
       is for assistive technology only. -->
  <h2 class="sr-only">{title}</h2>

  <!-- One bar over the list, as in the Changes panel: what is here, which of
       it is listed, and the verbs that act on all of it behind one menu.
       Three buttons used to stand over every comment; a menu is for
       choosing, and here there is something to choose. -->
  {#if headed}
    <header class="comments-header">
      <span class="comments-count panel-meta">{meta}</span>
      <div class="header-end">
        {#if onhistory}
          <button type="button" class="text-action" onclick={onhistory}>{m.comments_show_changes()}</button>
        {/if}
        {#if filtered.length}
          <!-- One of three, so the choices are radio items: the tick is for
               the eye, and the checked state says the same to a screen reader. -->
          <Menu positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}>
            <MenuIconButton icon="filter" size="1rem" class="filter-trigger"
                            pressed={status !== "all"}
                            label={m.comments_filter_label({ name: statusLabel })} />
            <ExplorerMenu>
              {#each STATUSES as option (option.value)}
                <Menu.OptionItem type="radio" value={option.value} checked={status === option.value}
                                 onCheckedChange={() => (status = option.value)} class="menuitem">
                  <span class="menuitem-check" aria-hidden="true">{status === option.value ? "✓" : ""}</span>
                  <span class="menuitem-label">{option.label}</span>
                </Menu.OptionItem>
              {/each}
            </ExplorerMenu>
          </Menu>
        {/if}
        {#if bulkOffered}
          <Menu onSelect={(chosen) => bulk(chosen.value)}
                positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}>
            <MenuIconButton icon="more-horizontal" size="1rem" class="bulk-trigger"
                            label={partial ? m.comments_group_loaded() : m.comments_group_all()} />
            <ExplorerMenu>
              {#if resolvable.length}
                <Menu.Item value="resolve" class="menuitem">
                  <span class="menuitem-label">{partial ? m.comments_resolve_loaded() : m.comments_resolve_all()}</span>
                </Menu.Item>
              {/if}
              {#if resolvable.length && clearable.length}
                <hr class="hr my-1" />
              {/if}
              {#if clearable.length}
                <Menu.Item value="clear" class="menuitem" disabled={!ondeletemany}>
                  <span class="menuitem-label">{m.comments_clear()}</span>
                </Menu.Item>
              {/if}
            </ExplorerMenu>
          </Menu>
        {/if}
      </div>
    </header>
  {/if}

  <!-- Keyed on the draft: the composer keeps what is being typed in its own
       state, so a second draft has to be a second component rather than the
       first one handed a new passage to be about. -->
  {#snippet composer()}
    {#key composing.id}
    <CommentComposer
      pending={composing.pending}
      bind:body={draftBody}
      {identity}
      {commentingAs}
      {needsLogin}
      {signInHref}
      onsend={(written) => onsend?.(written)}
      oncancel={() => oncancel?.()}
    />
    {/key}
  {/snippet}

  <div id="{cardIdPrefix}-list" class="comments-list flex flex-col gap-2" bind:this={list}>
    {@render pending?.()}
    {#each shown as comment (cardKey(comment))}
      {#if composerKey === cardKey(comment)}{@render composer()}{/if}
      <CommentCard
        {canComment}
        {comment}
        cardIdPrefix={cardIdPrefix}
        selected={String(comment.id) === String(selected)}
        {identity}
        {commentingAs}
        {canModerate}
        {onreveal}
        {onresolve}
        {ondelete}
        {onreply}
        {onedit}
        {onagent}
        {onloadreplies}
        {onattach}
        {attachable}
        onhold={(on) => hold(cardKey(comment), on)}
      />
    {/each}
    {#if composerKey === ""}{@render composer()}{/if}

    <!-- There are comments here, and the filter is hiding every one. -->
    {#if filtered.length && !shown.length && !composing}
      <p class="panel-muted" role="status">{m.comments_filter_empty()}</p>
    {/if}

    <!-- What lies past the cards. Four states, and they are not the same
         thing: still arriving, arrived and empty, holding a prefix, and
         holding the lot. A failed request keeps every card above it
         exactly where it is and offers the request again. Empty is said per
         tab: a document with comments and no highlights has an empty
         Highlights tab. -->
    {#if page}
      {#if page.error}
        <p class="page-error" role="alert">
          <span>{page.error}</span>
          <button type="button" class="btn btn-sm lp-control-tonal-brand"
            disabled={page.busy}
            onclick={() => onloadmore?.()}>{m.common_try_again()}</button>
        </p>
      {/if}
      {#if page.loading}
        <p class="panel-muted" role="status">{m.comments_loading_list()}</p>
      {:else if !page.total || (!partial && !filtered.length)}
        <p class="panel-muted" role="status">{empty}</p>
      {:else if partial}
        <button type="button" class="load-more btn btn-sm lp-control-tonal-brand"
          disabled={page.busy}
          onclick={() => onloadmore?.()}>
          {page.busy ? m.common_loading() : m.comments_load_more({ count: remaining })}
        </button>
      {/if}
    {/if}
  </div>

  <!-- How a comment gets made, at the foot of the panel where the Changes
       panel keeps its own hint. It is not an empty state -- someone who has
       read one comment still has to be told where the next one comes from --
       so it stays put, and down here it stays out of the comments' way. A
       remark about the whole document is the one kind that starts here
       rather than from a selection. -->
  <footer class="comments-hint">
    <p>{canComment ? hint : m.comments_read_only()}</p>
    {#if canComment && onwholedocument}
      <button type="button" class="text-action" onclick={() => onwholedocument?.()}>{m.common_comment_whole_document()}</button>
    {/if}
  </footer>
</div>

<style>
  /* The pane the collaboration panel puts this in carries the padding. The
     bar, the list and the hint bleed into it, so their dividers and the
     list's scrollbar run to the panel's edges, and each takes the gutter back
     as padding so everything shares one left edge. Nothing here clips: the
     pane already does. */
  .comments-panel { display: flex; flex: 1 1 0; min-height: 0; flex-direction: column; }
  .comments-panel > :global(*) { flex-shrink: 0; }
  .comments-header { display: flex; align-items: center; justify-content: space-between; gap: .5rem; min-width: 0; min-height: 2rem; margin-inline: calc(var(--panel-padding) * -1); padding: 0 var(--panel-padding) .4rem; border-bottom: 1px solid var(--color-divider); }
  .comments-count { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .header-end { display: inline-flex; flex: none; align-items: center; gap: .125rem; margin-inline-start: auto; }
  .comments-header :global(:is(.filter-trigger, .bulk-trigger)) { width: 2rem; height: 2rem; }
  /* The last trigger's icon, not its hover box, shares the right edge with the cards. */
  .header-end > :global(:is(.filter-trigger, .bulk-trigger):last-child) { margin-inline-end: -.5rem; }
  /* A filter that is hiding something wears the ground of a state that is on. */
  .comments-header :global(.filter-trigger[aria-pressed="true"]) { background: var(--color-row-selected); }
  .header-end > .text-action { margin-inline-end: .375rem; }
  .comments-list { flex: 1 1 auto; min-height: 0; margin-inline: calc(var(--panel-padding) * -1); padding: calc(var(--spacing) * 3) var(--panel-padding); overflow-y: auto; overscroll-behavior: contain; }
  .load-more { align-self: center; margin: calc(var(--spacing) * 2) 0; }
  .page-error { padding: calc(var(--spacing) * 2); display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); align-items: flex-start; }
  .comments-hint { margin: 0 calc(var(--panel-padding) * -1) calc(var(--panel-padding) * -1); padding: .5rem var(--panel-padding); border-top: 1px solid var(--color-divider); color: var(--panel-muted); font-size: var(--panel-meta-size); text-align: center; }
  .comments-hint p { margin: 0; }
  /* The same quiet text button the Changes panel uses for "Select all". */
  .text-action { display: inline-flex; align-items: center; min-height: 2rem; padding: 0; border: 0; background: none; color: var(--color-link, var(--color-brand)); font-size: var(--panel-meta-size); font-weight: 500; cursor: pointer; }
  .text-action:hover { text-decoration: underline; }
  .text-action:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px; }
  /* Phones: the panel's buttons, including Load more, are finger-sized. */
  @media (max-width: 767px) {
    .comments-panel .btn,
    .load-more,
    .text-action { min-height: 2.75rem; }
    .comments-header :global(:is(.filter-trigger, .bulk-trigger)) { width: 2.75rem; height: 2.75rem; }
  }
</style>
