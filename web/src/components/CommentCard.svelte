<script>
  import Row from "./layout/Row.svelte";

  // One annotation, and everything said about it.
  let {
    comment,
    identity = "",
    // The name the server would give this reply if nobody is signed in: the
    // account name when there is one, otherwise the per-document pseudonym.
    commentingAs = "Anonymous",
    canModerate = false,
    // The manifest entry at which this comment's passage stopped being found,
    // when it has been looked up and there was an answer. Null otherwise, and
    // null for every comment whose passage is still in the document.
    went = null,
    onreveal,
    onresolve,
    ondelete,
    onreply,
  } = $props();

  // A passage can be a paragraph long, which would bury the comment made about
  // it. The card shows the opening words and expands on request.
  const QUOTE_WORDS = 8;

  // A resolved note is settled business: it collapses to one line and stays
  // out of the way until someone clicks it open again. Resolving it again
  // closes it, however it was left.
  let expanded = $state(false);
  let quoteOpen = $state(false);
  let replying = $state(false);
  let replyBody = $state("");
  const replyName = $derived(identity || commentingAs);

  const collapsed = $derived(Boolean(comment.resolved) && !expanded);
  $effect(() => {
    if (!comment.resolved) expanded = false;
  });

  const words = $derived((comment.exact || "").split(/\s+/));
  const long = $derived(words.length > QUOTE_WORDS + 2);
  const short = $derived(words.slice(0, QUOTE_WORDS).join(" "));

  // The Delete button is only ever real for a comment the server says this
  // caller may delete, one they just posted and is still waiting to be
  // confirmed, or a caller who owns the document outright.
  const deletable = $derived(
    Boolean(comment.deletable) || Boolean(comment.temp_id) || Boolean(comment.pending) || canModerate,
  );

  const stamp = (value) => (value || "").replace("T", " ").slice(0, 16) + " UTC";

  // The one line a resolved card shows: what it was about, then what was said
  // about it.
  const summary = $derived(
    [
      comment.region ? `Figure ${comment.region.image_index + 1}` : (comment.exact || "").trim(),
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

  function submitReply(event) {
    event.preventDefault();
    if (!replyBody.trim()) return;
    onreply?.(comment, replyBody, replyName);
    replyBody = "";
  }
</script>

<!-- The whole card is the click target for opening a resolved note, which is
     what `summary` says it is. It is not a button: it holds buttons, and a
     button inside a button is a worse thing than a click handler on an
     article. The keyboard reaches everything in it through those. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
<article
  id="comment-{comment.id}"
  class="card cursor-pointer p-3 {comment.resolved || comment.pending
    ? 'preset-outlined-surface-200-800 opacity-70'
    : 'preset-outlined-surface-300-700'}"
  class:collapsed
  onclick={click}
>
  {#if collapsed}
    <div class="summary">{summary}</div>
  {:else}
    <div class="flex flex-col gap-2">
      <Row gap={1} wrap>
        {#if comment.orphaned}
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
          <span class="badge preset-tonal-tertiary">{comment.motivation}</span>
        {/if}
      </Row>

      {#if comment.region}
        <blockquote class="figureref panel-muted">
          Figure {comment.region.image_index + 1}
        </blockquote>
      {:else if long}
        <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3">
          <span>{quoteOpen ? `“${comment.exact}”` : `“${short}`}</span>
          <!-- svelte-ignore a11y_invalid_attribute -->
          <a
            href="#"
            class="anchor"
            onclick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              quoteOpen = !quoteOpen;
            }}
          >
            {quoteOpen ? " less" : "… ”"}
          </a>
        </blockquote>
      {:else}
        <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3">
          “{comment.exact}”
        </blockquote>
      {/if}

      {#if comment.source}
        <!-- The anchor of record, quietly: which file the quotation above
             was actually cut from. -->
        <div class="text-surface-500 text-xs">{comment.source.path}</div>
      {/if}

      <!-- The quotation above is what the passage said when the comment was
           made. This is what became of it: the moment it stopped being in the
           document, by the name somebody gave that moment or by its digest.
           What replaced it is the word-level diff, and is not built. -->
      {#if went}
        <p class="panel-muted">
          Removed in {went.label || went.sha.slice(0, 7)}, {new Date(went.at).toLocaleDateString()}.
        </p>
      {/if}

      {#if comment.body}<p>{comment.body}</p>{/if}

      <small class="panel-meta">{comment.creator} · {stamp(comment.created)}</small>

      {#if comment.replies?.length}
        <ul class="border-surface-200-800 flex flex-col gap-2 border-l pl-3">
          {#each comment.replies as reply (reply.id)}
            <li>
              <span>{reply.body}</span><br />
              <small class="panel-meta">{reply.creator} · {stamp(reply.created)}</small>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <Row gap={1} justify="end">
    <button
      type="button"
      class="btn btn-sm preset-outlined-surface-300-700"
      onclick={(e) => {
        e.stopPropagation();
        expanded = false;
        onresolve?.(comment);
      }}
    >
      {comment.resolved ? "Reopen" : "Resolve"}
    </button>
    <button
      type="button"
      class="btn btn-sm preset-outlined-surface-300-700"
      onclick={(e) => {
        e.stopPropagation();
        replying = !replying;
      }}
    >
      Reply
    </button>
    {#if deletable}
      <button
        type="button"
        class="btn btn-sm preset-outlined-error-500"
        onclick={(e) => {
          e.stopPropagation();
          ondelete?.(comment);
        }}
      >
        Delete
      </button>
    {/if}
  </Row>

  {#if replying}
    <form class="mt-3 flex flex-col gap-2" onsubmit={submitReply}>
      {#if !identity}
        <p class="panel-meta">replying as {commentingAs}</p>
      {/if}
      <!-- The box exists because somebody just clicked Reply, so the caret
           belongs in it. The rule is about a page that takes the focus on
           load, which this is not. -->
      <!-- svelte-ignore a11y_autofocus -->
      <textarea
        class="textarea"
        placeholder="Reply"
        rows="2"
        maxlength="5000"
        required
        autofocus
        bind:value={replyBody}
      ></textarea>
      <button type="submit" class="btn btn-sm preset-filled-primary-500 self-end">Add reply</button>
    </form>
  {/if}
</article>
