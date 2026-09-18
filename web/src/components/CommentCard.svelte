<script>
  import { tick } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import IconButton from "./IconButton.svelte";
  import Avatar from "./Avatar.svelte";
  import Row from "./layout/Row.svelte";
  import * as history from "../lib/history.js";
  import { runsFor } from "../lib/suggestions.js";
  import { threadRuns } from "../lib/thread.js";
  import { shownSelector } from "../lib/anchor.js";
  import { day as isoDay, moment } from "../lib/dates.js";

  // One annotation, and everything said about it.
  let {
    comment,
    identity = "",
    // The name the server would give this reply if nobody is signed in: the
    // account name when there is one, otherwise the per-document pseudonym.
    commentingAs = "Anonymous",
    canModerate = false,
    canComment = true,
    // The manifest entry at which this comment's passage stopped being found,
    // when it has been looked up and there was an answer. Null otherwise, and
    // null for every comment whose passage is still in the document.
    went = null,
    // The inserted side of the word diff when the quoted passage was
    // replaced. It is supplied by the page, which owns historical fetches.
    replacement = null,
    onreveal,
    onresolve,
    ondelete,
    onreply,
    // A suggestion's own two verbs. Hidden entirely for any other comment.
    onaccept,
    onreject,
    cardIdPrefix = "comment",
    // The card the sidebar has singled out: its passage is ringed in the
    // document, and the card wears the same ring so the two read as one.
    selected = false,
  } = $props();

  // What this comment looked like where it was made. Display evidence: it
  // is what the card shows and what the highlight was drawn from, never what
  // the comment is about.
  const shown = $derived(shownSelector(comment));

  // A suggestion is a comment whose motivation is `editing` (the W3C term).
  const isSuggestion = $derived(comment.motivation === "editing");
  const annotationColor = $derived(/^#[0-9a-f]{6}$/i.test(comment.color || "") ? comment.color : null);

  // The word-level diff between the quoted passage and its proposal, kept in
  // a rune rather than computed inline: `history.wordDiff` is async (it runs
  // the shared WASM renderer worker), so it is asked for in an effect and
  // turned into displayable runs by the pure half of this, `runsFor`, which
  // is what `web/tests/unit/suggestions.mjs` actually exercises.
  let diffRuns = $state([]);
  let diffRequest = 0;
  $effect(() => {
    if (!isSuggestion) {
      diffRuns = [];
      return;
    }
    // The passage a suggestion replaces, as the source has it: that is what
    // the proposal is a replacement for, and what the diff is against.
    const oldText = comment.original_anchor?.target?.exact ?? shown.exact;
    const proposedText = comment.proposed ?? "";
    const request = ++diffRequest;
    history.wordDiff(oldText, proposedText).then((edits) => {
      if (request !== diffRequest) return; // superseded by a newer comment or proposal
      diffRuns = runsFor(oldText, edits);
    });
  });

  // A resolved note is settled business: it collapses to one line and stays
  // out of the way until someone clicks it open again. Resolving it again
  // closes it, however it was left.
  let expanded = $state(false);
  let card = $state(null);
  let replying = $state(false);
  let replyBody = $state("");
  let replyField = $state(null);
  const replyName = $derived(identity || commentingAs);

  const collapsed = $derived(Boolean(comment.resolved) && !expanded);
  $effect(() => {
    if (!comment.resolved) expanded = false;
  });

  // The Delete button is only ever real for a comment the server says this
  // caller may delete, one they just posted and is still waiting to be
  // confirmed, or a caller who owns the document outright.
  const deletable = $derived(
    Boolean(comment.deletable) || Boolean(comment.temp_id) || Boolean(comment.pending) || canModerate,
  );

  const stamp = (value) => (value || "").replace("T", " ").slice(0, 16) + " UTC";
  // The one line a resolved card shows: what it was about, then what was said
  // about it. A decided suggestion leads with the decision, since that is
  // what "resolved" means for it.
  const summary = $derived(
    [
      comment.outcome === "accepted" ? "Accepted" : comment.outcome === "rejected" ? "Rejected" : "",
      shown.point ? "Point comment" : shown.exact.trim(),
      (comment.body || "").trim(),
    ]
      .filter(Boolean)
      .join(" — ") || "Resolved",
  );

  // The note and its replies as one conversation, run together by author.
  // Repeating the badge on every line makes a stack of identities out of what
  // is really a short exchange between two or three people, and in a sidebar
  // this narrow the repeated gutter costs more than the reply is worth. The
  // rule itself, and why the run is keyed on a display name, live in
  // `lib/thread.js` -- `web/tests/unit/thread.mjs` is what exercises it.
  const runs = $derived(threadRuns(comment));

  // Opening a resolved note replaces the summary button with the card, so the
  // element that was activated stops existing. Focus would be dropped to the
  // body there, losing a keyboard reader's place in the column; the card
  // takes it instead, which is where the words they just asked for are.
  async function open() {
    expanded = true;
    await tick();
    card?.focus({ preventScroll: true });
  }

  function click(event) {
    if (event.target.closest("button,input,textarea,a")) return;
    // Collapsed, the click is "show me this again"; open, it is "take me to
    // the place in the document this is about".
    if (collapsed) {
      open();
      return;
    }
    if (!comment.orphaned) onreveal?.(comment);
  }

  async function reply() {
    replying = true;
    await tick();
    replyField?.focus();
  }

  // No buttons under the reply box: Enter sends, Shift+Enter breaks the line
  // and Escape puts the box away, as in the chat composer.
  function replyKey(event) {
    if (event.key === "Escape") {
      event.preventDefault();
      replyBody = "";
      replying = false;
      return;
    }
    if (event.key !== "Enter" || event.shiftKey || event.ctrlKey || event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  }

  function submitReply(event) {
    event.preventDefault();
    if (!replyBody.trim()) return;
    onreply?.(comment, replyBody, replyName);
    replyBody = "";
    replying = false;
  }
</script>

<!-- The whole card is the click target for opening a resolved note, which is
     what `summary` says it is. It is not a button: it holds buttons, and a
     button inside a button is a worse thing than a click handler on an
     article. The keyboard reaches everything in it through those -- except
     when it is collapsed, where the card holds no buttons at all and the
     click was the only way in. There, the summary is the button. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
<article
  id="{cardIdPrefix}-{comment.id}"
  class="card cursor-pointer p-2 {comment.resolved || comment.pending
    ? 'preset-outlined-surface-200-800 opacity-70'
    : 'preset-outlined-surface-300-700'}"
  class:collapsed
  class:selected
  bind:this={card}
  tabindex="-1"
  onfocus={() => (expanded = true)}
  onclick={click}
>
  {#if collapsed}
    <button type="button" class="summary" aria-expanded="false"
            onclick={open}>{summary}</button>
  {:else}
    <div class="flex flex-col gap-2">
      <Row gap={1} wrap>
        {#if comment.earlierPublication}
          <span class="badge preset-tonal-warning">Earlier published version</span>
        {/if}
        {#if comment.orphaned}
          <span class="badge preset-tonal-warning">
            {comment.attachment?.diagnostic === "removed_file" ? "Source file removed"
              : comment.attachment?.status === "ambiguous" ? "Source passage ambiguous"
              : comment.attachment?.status === "unresolved" ? "Source passage unresolved"
              : comment.attachment?.status === "deleted" || went ? "Passage removed"
              : "Passage not on this page"}
          </span>
        {/if}
        {#if comment.inSourceOnly}
          <!-- The rendering lost the words, but the source -- the text that
               is actually versioned -- still has them, so this is not lost,
               only unreachable from the page. -->
          <span class="badge preset-tonal-secondary">In the source, not on the page</span>
        {/if}
        <!-- The motivation is the W3C annotation type. Commenting is the
             default, so only the others are worth showing. -->
        {#if comment.motivation && comment.motivation !== "commenting"}
          <span class="badge preset-tonal-tertiary">{isSuggestion ? "Suggested change" : "Highlight"}</span>
        {/if}
      </Row>

      <!-- The passage itself is not repeated here: it is painted in the
           document, and ringed there while this card is the selected one.
           Only a note with no words to point at says what it is on. -->
      {#if shown.point}
        <p class="panel-muted">Comment at this point</p>
      {/if}

      {#if isSuggestion}
        <!-- The word-level diff of the quotation against the proposal:
             deletion struck through, insertion underlined. Deletion in full
             when the proposal is empty falls out of the diff itself. -->
        <p class="suggestion-diff border-primary-500 border-l-2 pl-3 text-sm">
          {#each diffRuns as run, i (i)}
            {#if run.kind === "del"}<del>{run.text}</del>
            {:else if run.kind === "ins"}<ins>{run.text}</ins>
            {:else}{run.text}{/if}
          {/each}
        </p>
        {#if comment.outcome === "accepted"}
          <p class="panel-muted">Accepted in {history.shortSha(comment.resolved_in)}.</p>
        {:else if comment.outcome === "rejected"}
          <p class="panel-muted">Rejected.</p>
        {:else if canModerate}
          <Row gap={2}>
            <button
              type="button"
              class="btn btn-sm preset-filled-primary-500"
              disabled={Boolean(comment.deciding)}
              onclick={(e) => {
                e.stopPropagation();
                onaccept?.(comment);
              }}
            >{comment.deciding === "accept" ? "Accepting…" : "Accept"}</button>
            <button
              type="button"
              class="btn btn-sm preset-outlined-surface-300-700"
              disabled={Boolean(comment.deciding)}
              onclick={(e) => {
                e.stopPropagation();
                onreject?.(comment);
              }}
            >{comment.deciding === "reject" ? "Rejecting…" : "Reject"}</button>
          </Row>
        {/if}
      {/if}

      <!-- The quotation above is what the passage said when the comment was
           made. This is what became of it: the moment it stopped being in the
           document, and the inserted side of the shared word-level diff. -->
      {#if went}
        <p class="panel-muted">
          Removed in {went.label || went.sha.slice(0, 7)}, {isoDay(went.at)}.
        </p>
      {/if}
      {#if replacement !== null}
        <p class="panel-muted">Now: {replacement ? `“${replacement}”` : "deleted without replacement."}</p>
      {/if}

      <div class="thread">
        {#each runs as run (run.id)}
          <div class="run">
            <div class="run-head">
              <Avatar name={run.author} size={5} title={`${run.author} · ${stamp(run.created)}`} />
              <strong class="run-author">{run.author}</strong>
              <small class="panel-meta">{moment(run.created)}</small>
            </div>
            {#each run.posts as post (post.id)}
              {#if post.body}<p class="post" title={stamp(post.created)}>{post.body}</p>{/if}
            {/each}
          </div>
        {/each}
      </div>
    </div>
  {/if}

  <!-- One row of verbs for the whole thread, and it keeps out of the way:
       the two that get used stay visible only while the card is the one being
       read, and the one that cannot be undone waits behind the menu. A card
       that is hovered, focused into, or selected shows them; so does every
       card on a device with no pointer to hover with. -->
  <div class="actions">
    {#if !isSuggestion && canComment}
      <!-- Reject is the resolve, for a suggestion -- see the diff and
           decision block above. -->
      <IconButton
        icon="check"
        label={comment.resolved ? "Reopen" : "Resolve"}
        pressed={Boolean(comment.resolved)}
        onclick={(e) => {
          e.stopPropagation();
          expanded = false;
          onresolve?.(comment);
        }}
      />
    {/if}
    <IconButton
      icon="reply"
      label="Reply"
      tone={canComment ? "outlined" : null}
      disabled={!canComment}
      onclick={(e) => {
        e.stopPropagation();
        void reply();
      }}
    />
    {#if deletable}
      <Menu onSelect={(chosen) => { if (chosen.value === "delete") ondelete?.(comment); }}>
        <!-- The button is authored here rather than handed a `class`: a class
             arriving as a prop carries no scope hash, so the rules below would
             have to be global to paint at all. -->
        <Menu.Trigger>
          {#snippet element(attributes)}
            <button {...attributes} type="button" class="comment-menu" aria-label="More comment options">•••</button>
          {/snippet}
        </Menu.Trigger>
        <ExplorerMenu>
          <Menu.Item value="delete" class="menuitem">Delete comment</Menu.Item>
        </ExplorerMenu>
      </Menu>
    {/if}
  </div>

  {#if replying && canComment}
    <form class="mt-2 flex flex-col gap-2" onsubmit={submitReply}>
      {#if !identity}
        <p class="panel-meta">replying as {commentingAs}</p>
      {/if}
      <textarea
        class="textarea"
        aria-label="Reply"
        placeholder="Reply · Enter sends · Esc cancels"
        rows="2"
        maxlength="5000"
        required
        bind:this={replyField}
        bind:value={replyBody}
        onkeydown={replyKey}
      ></textarea>
    </form>
  {/if}
</article>

<style>
  .selected { border-color: var(--color-primary-500); box-shadow: 0 0 0 1px var(--color-primary-500); }

  /* The conversation: runs of one author separated by a little air, the lines
     within a run tight enough to read as one person still talking. No rail
     down the side and no avatar gutter -- a long reply in a sidebar this
     narrow needs the whole width. */
  .thread { display: flex; flex-direction: column; gap: calc(var(--spacing) * 3); }
  .run { display: flex; flex-direction: column; gap: calc(var(--spacing) * 0.75); }
  .run-head { display: flex; align-items: center; gap: var(--spacing); min-width: 0; }
  .run-author { min-width: 0; overflow: hidden; font-size: var(--panel-meta-size); line-height: var(--panel-line-height); text-overflow: ellipsis; white-space: nowrap; }
  .post { margin: 0; overflow-wrap: anywhere; white-space: pre-wrap; }

  .actions { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 0.5); margin-top: var(--spacing); opacity: 0; transition: opacity 120ms ease-in-out; }
  /* Visible whenever the card is the one in hand. `:focus-within` is what
     keeps them reachable from the keyboard: they are always in the tab order,
     and tabbing to one is what paints it. */
  .card:hover .actions,
  .card:focus-within .actions,
  .selected .actions { opacity: 1; }
  /* Nothing hovers on a touch screen, so there they simply stay. */
  @media (hover: none) { .actions { opacity: 1; } }
  @media (prefers-reduced-motion: reduce) { .actions { transition: none; } }

  .comment-menu { padding-inline: calc(var(--spacing) * 1.5); border-radius: var(--radius-base); font-size: var(--panel-meta-size); line-height: 1; color: var(--panel-muted); }
  .comment-menu:hover, .comment-menu[data-state="open"] { background: var(--color-row-hover); }
</style>
