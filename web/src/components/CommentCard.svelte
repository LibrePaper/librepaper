<script>
  import { tick } from "svelte";
  import IconButton from "./IconButton.svelte";
  import Avatar from "./Avatar.svelte";
  import DictationButton from "./DictationButton.svelte";
  import { textareaTarget } from "../lib/dictation/targets.js";
  import Row from "./layout/Row.svelte";
  import * as history from "../lib/history.js";
  import { runsFor } from "../lib/suggestions.js";
  import { day as isoDay } from "../lib/dates.js";

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
    const oldText = comment.source?.exact ?? comment.exact ?? "";
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
      comment.point ? "Point comment" : comment.region ? `Figure ${comment.region.image_index + 1}` : (comment.exact || "").trim(),
      (comment.body || "").trim(),
    ]
      .filter(Boolean)
      .join(" — ") || "Resolved",
  );

  function click(event) {
    if (event.target.closest("button,input,textarea,a")) return;
    // Collapsed, the click is "show me this again"; open, it is "take me to
    // the place in the document this is about".
    if (collapsed) {
      expanded = true;
      return;
    }
    if (!comment.orphaned && !comment.regionUnplaceable) onreveal?.(comment);
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
     article. The keyboard reaches everything in it through those. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
<article
  id="{cardIdPrefix}-{comment.id}"
  class="card cursor-pointer p-3 {comment.resolved || comment.pending
    ? 'preset-outlined-surface-200-800 opacity-70'
    : 'preset-outlined-surface-300-700'}"
  class:collapsed
  class:selected
  tabindex="-1"
  onfocus={() => (expanded = true)}
  onclick={click}
>
  {#if collapsed}
    <div class="summary">{summary}</div>
  {:else}
    <div class="flex flex-col gap-2">
      <Row gap={1} wrap>
        {#if comment.output_anchor}
          <span class="badge preset-tonal-surface">Current output anchor</span>
        {/if}
        {#if comment.earlierPublication}
          <span class="badge preset-tonal-warning">Earlier published version</span>
        {:else if comment.orphaned}
          <!-- "Needs re-anchoring" said what the machine could not do. This
               says what happened to the words, which is what the person who
               wrote the comment came back to find out. -->
          <span class="badge preset-tonal-warning">
            {went ? "Passage removed" : "Passage not in the document"}
          </span>
        {/if}
        {#if comment.inSourceOnly}
          <!-- The rendering lost the words, but the source -- the text that
               is actually versioned -- still has them, so this is not lost,
               only unreachable from the page. -->
          <span class="badge preset-tonal-secondary">In the source, not on the page</span>
        {/if}
        {#if comment.regionUnplaceable}
          <span class="badge preset-tonal-warning">
            {comment.regionUnplaceableReason === "pdf"
              ? "Figure region unavailable in this PDF"
              : "Figure unavailable in this version"}
          </span>
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
      {#if comment.point}
        <p class="panel-muted">Comment at this point</p>
      {:else if comment.region}
        <blockquote class="figureref panel-muted">
          Figure {comment.region.image_index + 1}
        </blockquote>
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
        {#if !comment.source}
          <p class="panel-muted text-xs">
            No source anchor: an editor will have to apply this by hand.
          </p>
        {/if}
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

      {#if comment.body}<p>{comment.body}</p>{/if}


      <div class="byline">
        <Avatar name={comment.creator} size={5} title={`${comment.creator} · ${stamp(comment.created)}`} />
        <small class="panel-meta">{stamp(comment.created)}</small>
      </div>

      {#if comment.replies?.length}
        <ul class="border-surface-200-800 flex flex-col gap-2 border-l pl-3">
          {#each comment.replies as reply (reply.id)}
            <li class="reply">
              <Avatar name={reply.creator} size={5} title={`${reply.creator} · ${stamp(reply.created)}`} />
              <span>{reply.body}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <Row gap={1} justify="end">
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
      disabled={!canComment}
      onclick={async (e) => {
        e.stopPropagation();
        replying = true;
        await tick();
        replyField?.focus();
      }}
    />
    {#if deletable}
      <IconButton
        icon="trash"
        label="Delete"
        colour="text-error-500"
        onclick={(e) => {
          e.stopPropagation();
          ondelete?.(comment);
        }}
      />
    {/if}
  </Row>

  {#if replying && canComment}
    <form class="mt-3 flex flex-col gap-2" onsubmit={submitReply}>
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
      <Row gap={2} justify="end">
        <DictationButton target={() => textareaTarget(replyField)} label="Dictate reply" size="btn-icon-sm" />
      </Row>
    </form>
  {/if}
</article>

<style>
  .selected { border-color: var(--color-primary-500); box-shadow: 0 0 0 1px var(--color-primary-500); }
  .byline { display: flex; align-items: center; gap: var(--spacing); }
  .reply { display: flex; align-items: flex-start; gap: var(--spacing); }
  .reply > span { min-width: 0; overflow-wrap: anywhere; }
</style>
