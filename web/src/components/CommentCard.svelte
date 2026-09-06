<script>
  import Row from "./layout/Row.svelte";
  import { AUTHOR, read, write } from "../lib/storage.js";

  // One annotation, and everything said about it.
  let {
    comment,
    identity = "",
    canModerate = false,
    // The manifest entry at which this comment's passage stopped being found,
    // when it has been looked up and there was an answer. Null otherwise, and
    // null for every comment whose passage is still in the document.
    went = null,
    ontoggleTag,
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
  // What an anonymous reader is called, which is theirs to type. Held apart
  // from `identity` rather than seeded from it: the account arrives a moment
  // after this card is built, and a name captured before it landed would leave
  // a signed-in reader replying as "Anonymous" for the rest of the session.
  let typed = $state(read(AUTHOR, "Anonymous"));
  const replyName = $derived(identity || typed);

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
    if (!comment.orphaned) onreveal?.(comment);
  }

  function submitReply(event) {
    event.preventDefault();
    if (!replyBody.trim()) return;
    if (!identity) write(AUTHOR, typed);
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
        <!-- The motivation is the W3C annotation type. Commenting is the
             default, so only the others are worth showing. -->
        {#if comment.motivation && comment.motivation !== "commenting"}
          <span class="badge preset-tonal-tertiary">{comment.motivation}</span>
        {/if}
      </Row>

      {#if comment.region}
        <blockquote class="figureref text-surface-600-400 text-sm">
          Figure {comment.region.image_index + 1}
        </blockquote>
      {:else if long}
        <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3 text-sm">
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
        <blockquote class="border-primary-500 text-surface-700-300 border-l-2 pl-3 text-sm">
          “{comment.exact}”
        </blockquote>
      {/if}

      <!-- The quotation above is what the passage said when the comment was
           made. This is what became of it: the moment it stopped being in the
           document, by the name somebody gave that moment or by its digest.
           What replaced it is the word-level diff, and is not built. -->
      {#if went}
        <p class="text-surface-600-400 text-sm">
          Removed in {went.label || went.sha.slice(0, 7)}, {new Date(went.at).toLocaleDateString()}.
        </p>
      {/if}

      {#if comment.body}<p class="text-sm">{comment.body}</p>{/if}

      {#if comment.tags?.length}
        <Row gap={1} wrap>
          {#each comment.tags as tag}
            <button
              type="button"
              class="chip preset-outlined-surface-300-700 text-xs"
              onclick={(e) => {
                e.stopPropagation();
                ontoggleTag?.(tag);
              }}
            >
              {tag}
            </button>
          {/each}
        </Row>
      {/if}

      <small class="text-surface-500 text-xs">{comment.creator} · {stamp(comment.created)}</small>

      {#if comment.replies?.length}
        <ul class="border-surface-200-800 flex flex-col gap-2 border-l pl-3">
          {#each comment.replies as reply (reply.id)}
            <li class="text-sm">
              <span>{reply.body}</span><br />
              <small class="text-surface-500 text-xs">{reply.creator} · {stamp(reply.created)}</small>
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
        <input class="input" placeholder="Name" maxlength="80" bind:value={typed} />
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
