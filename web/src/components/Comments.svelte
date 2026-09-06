<script>
  import IconButton from "./IconButton.svelte";
  import CommentCard from "./CommentCard.svelte";
  import Row from "./layout/Row.svelte";

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
    tool = "commenting",
    hasFigures = false,
    // Where each orphaned passage went, by comment id. Passed through rather
    // than looked up here: the card is what says it, and the page is what
    // knows it.
    went = {},
    ontool,
    onreveal,
    onresolve,
    ondelete,
    onreply,
  } = $props();

  // An empty set shows everything; an annotation matches if it carries every
  // tag chosen.
  let chosen = $state(new Set());

  const allTags = $derived.by(() => {
    const all = new Set();
    for (const comment of comments) for (const tag of comment.tags || []) all.add(tag);
    return [...all].sort();
  });

  function toggleTag(tag) {
    const next = new Set(chosen);
    next.has(tag) ? next.delete(tag) : next.add(tag);
    chosen = next;
  }

  function place(comment) {
    if (comment.region) {
      const at = figureAt[comment.region.image_index];
      if (Number.isFinite(at)) return at;
    }
    return Number.isFinite(comment.start) ? comment.start : Infinity;
  }

  const shown = $derived(
    comments
      .filter((comment) => [...chosen].every((tag) => (comment.tags || []).includes(tag)))
      .sort((a, b) => place(a) - place(b) || a.seq - b.seq),
  );

  const open = $derived(comments.filter((comment) => !comment.resolved).length);

  const TOOLS = [
    { id: "commenting", icon: "comment", label: "Comment", title: "Comment on the selected passage" },
    { id: "highlighting", icon: "highlight", label: "Highlight", title: "Highlight, with no comment" },
    { id: "region", icon: "box", label: "Box", title: "Drag a box on a figure" },
  ];
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="panel">
  <!-- The tools belong with the comments they make, and stay in view while the
       pane scrolls. Box draws on a figure, so a document with no figures has
       nothing for it to do; saying so is better than a button that silently
       does nothing. -->
  <div class="toolrow sticky top-0 z-1 py-3">
    <div class="tools flex gap-1" role="radiogroup" aria-label="Annotation tool">
      {#each TOOLS as item}
        <IconButton
          icon={item.icon}
          label={item.label}
          tool={item.id}
          pressed={tool === item.id}
          disabled={item.id === "region" && !hasFigures}
          title={item.id === "region" && !hasFigures
            ? "This document has no figures to draw on"
            : item.title}
          onclick={() => ontool?.(item.id)}
        />
      {/each}
    </div>
  </div>

  <header class="mb-3">
    <Row justify="between">
      <h3 class="h5">Comments</h3>
      {#if comments.length}
        <small class="text-surface-600-400">{open} open · {comments.length} total</small>
      {/if}
    </Row>
    <!-- Onboarding, not a caption: it goes once there is something in the
         column to read. -->
    {#if comments.length === 0}
      <p class="text-surface-600-400 text-sm">
        Highlight text in the document, then choose “Comment”.
      </p>
    {/if}
  </header>

  {#if allTags.length}
    <div class="mb-3 flex flex-wrap gap-1">
      {#each allTags as tag}
        <button
          type="button"
          class="chip {chosen.has(tag) ? 'preset-filled-primary-500' : 'preset-outlined-surface-300-700'}"
          onclick={() => toggleTag(tag)}
        >
          {tag}
        </button>
      {/each}
    </div>
  {/if}

  <div id="comments" class="flex flex-col gap-3">
    {#each shown as comment (comment)}
      <CommentCard
        {comment}
        {identity}
        {commentingAs}
        {canModerate}
        went={went[comment.id] || null}
        ontoggleTag={toggleTag}
        {onreveal}
        {onresolve}
        {ondelete}
        {onreply}
      />
    {/each}
  </div>
</div>
