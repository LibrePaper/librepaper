<script>
  import IconButton from "./IconButton.svelte";
  import CommentCard from "./CommentCard.svelte";
  import PanelHeader from "./PanelHeader.svelte";

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
    // Inserted side of a historical word diff, keyed by comment id. The page
    // owns fetching and anchoring this data; cards only present it.
    replacements = {},
    ontool,
    onreveal,
    onresolve,
    ondelete,
    onreply,
    onaccept,
    onreject,
  } = $props();

  function place(comment) {
    if (comment.region) {
      const at = figureAt[comment.region.image_index];
      if (Number.isFinite(at)) return at;
    }
    return Number.isFinite(comment.start) ? comment.start : Infinity;
  }

  const shown = $derived([...comments].sort((a, b) => place(a) - place(b) || a.seq - b.seq));

  const open = $derived(comments.filter((comment) => !comment.resolved).length);

  const TOOLS = [
    { id: "commenting", icon: "comment", label: "Comment", title: "Comment on the selected passage" },
    { id: "highlighting", icon: "highlight", label: "Highlight", title: "Highlight, with no comment" },
    { id: "editing", icon: "pencil", label: "Suggest", title: "Suggest a replacement for the selected passage" },
    { id: "region", icon: "box", label: "Box", title: "Drag a box on a figure" },
  ];
</script>

<!-- One of the column's panels: the column itself, with the tabs that choose
     between them, is the reader's. -->
<div class="panel">
  <PanelHeader
    title="Comments"
    meta={comments.length ? `${open} open · ${comments.length} total` : undefined}
  >
    {#snippet actions()}
      <div class="tools flex gap-1" role="radiogroup" aria-label="Annotation tool">
      {#each TOOLS as item}
        <IconButton
          icon={item.icon}
          size="btn-icon-sm"
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
    {/snippet}
    {#if comments.length === 0}
      <p class="panel-muted">
        Highlight text in the document, then choose “Comment”.
      </p>
    {/if}
  </PanelHeader>

  <div id="comments" class="flex flex-col gap-3">
    {#each shown as comment (comment)}
      <CommentCard
        {comment}
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
</div>
