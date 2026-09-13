<script>
  import Chat from "../Chat.svelte";
  import Comments from "../Comments.svelte";

  let {
    messages = [], connected = false, canPost = false, onsend,
    comments = [], figureAt = [], identity = "", commentingAs = "Anonymous",
    canModerate = false, canComment = true, tool = "commenting", hasFigures = false,
    went = {}, replacements = {}, ontool, onreveal, onresolve, ondelete,
    ondeletemany, onreply, onaccept, onreject, onrejectconfirmed, pending,
    unreadChat = false, selected = "", tab = $bindable("comments"),
  } = $props();
  const VIEWS = [
    { id: "chat", label: "Chat" },
    { id: "comments", label: "Comments" },
    { id: "highlights", label: "Highlights" },
  ];
  const common = () => ({ figureAt, identity, commentingAs, canModerate, canComment, tool,
    hasFigures, went, replacements, ontool, onreveal, onresolve, ondelete, ondeletemany,
    onreply, onaccept, onreject, onrejectconfirmed, pending, selected });
</script>

<section class="collaboration panel" aria-label="Collaboration">
  <div class="panel-tabs collab-tabs" role="tablist" aria-label="Collaboration views">
    {#each VIEWS as view, index}
      <button id="collaboration-tab-{view.id}" aria-controls="collaboration-panel-{view.id}"
        title={view.label}
        class:active={tab === view.id} role="tab" aria-selected={tab === view.id} tabindex={tab === view.id ? 0 : -1}
        onkeydown={(event) => {
          const last = VIEWS.length - 1;
          const next = event.key === "ArrowRight" ? (index + 1) % VIEWS.length
            : event.key === "ArrowLeft" ? (index + last) % VIEWS.length
            : event.key === "Home" ? 0 : event.key === "End" ? last : null;
          if (next === null) return;
          event.preventDefault();
          tab = VIEWS[next].id;
          document.getElementById("collaboration-tab-" + tab)?.focus();
        }}
        onclick={() => (tab = view.id)}>{view.label}{#if view.id === "chat" && unreadChat}<span class="unread" aria-label="Unread messages"></span>{/if}</button>
    {/each}
  </div>
  <div id="collaboration-panel-chat" aria-labelledby="collaboration-tab-chat" role="tabpanel" hidden={tab !== "chat"} class="collab-panel"><Chat {messages} {connected} {canPost} {onsend} /></div>
  <div id="collaboration-panel-comments" aria-labelledby="collaboration-tab-comments" role="tabpanel" hidden={tab !== "comments"} class="collab-panel">
    <Comments {...common()} {comments} filter="comments" cardIdPrefix="collaboration-comment" />
  </div>
  <div id="collaboration-panel-highlights" aria-labelledby="collaboration-tab-highlights" role="tabpanel" hidden={tab !== "highlights"} class="collab-panel">
    <Comments {...common()} {comments} filter="highlights" title="Highlights"
      cardIdPrefix="collaboration-highlight"
      emptyMessage="Select text and choose Highlight. Add a reply to discuss a highlight." />
  </div>
</section>

<style>
  .collaboration { display:flex; min-height:0; flex-direction:column; overflow:hidden; padding:0; }
  .collab-panel { display:flex; flex:1 1 auto; min-height:0; flex-direction:column; overflow:hidden; }
  .collab-panel[hidden] { display:none; }
  /* Layout, truncation and the narrow-panel step live on .panel-tabs; the
     gutters stay there too, so this row and the agent panel's row compress at
     the same width instead of a gutter's worth apart. */
  .collab-tabs button { color:var(--color-surface-600-400); border-bottom:2px solid transparent; }
  .collab-tabs button.active { color:var(--color-primary-500); border-color:var(--color-primary-500); }
  .unread { display:inline-block; width:.45rem; height:.45rem; margin-left:.35rem; vertical-align:middle; border-radius:50%; background:var(--color-primary-500); }
</style>
