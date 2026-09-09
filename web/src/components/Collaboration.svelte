<script>
  import Chat from "./Chat.svelte";
  import Comments from "./Comments.svelte";

  let {
    messages = [], connected = false, canPost = false, onsend,
    comments = [], figureAt = [], identity = "", commentingAs = "Anonymous",
    canModerate = false, canComment = true, tool = "commenting", hasFigures = false,
    went = {}, replacements = {}, ontool, onreveal, onresolve, ondelete,
    ondeletemany, onreply, onaccept, onreject, onrejectconfirmed, pending,
    unreadChat = false, tab = $bindable("comments"),
  } = $props();
  const common = () => ({ figureAt, identity, commentingAs, canModerate, canComment, tool,
    hasFigures, went, replacements, ontool, onreveal, onresolve, ondelete, ondeletemany,
    onreply, onaccept, onreject, onrejectconfirmed, pending });
</script>

<section class="collaboration panel" aria-label="Collaboration">
  <div class="collab-tabs" role="tablist" aria-label="Collaboration views">
    {#each ["chat", "comments", "highlights"] as view, index}
      <button id="collaboration-tab-{view}" aria-controls="collaboration-panel-{view}"
        class:active={tab === view} role="tab" aria-selected={tab === view} tabindex={tab === view ? 0 : -1}
        onkeydown={(event) => {
          const views = ["chat", "comments", "highlights"];
          const next = event.key === "ArrowRight" ? (index + 1) % 3
            : event.key === "ArrowLeft" ? (index + 2) % 3
            : event.key === "Home" ? 0 : event.key === "End" ? 2 : null;
          if (next === null) return;
          event.preventDefault();
          tab = views[next];
          document.getElementById("collaboration-tab-" + tab)?.focus();
        }}
        onclick={() => (tab = view)}>{view[0].toUpperCase() + view.slice(1)}{#if view === "chat" && unreadChat}<span class="unread" aria-label="Unread messages"></span>{/if}</button>
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
  .collab-tabs { display:flex; gap:.25rem; padding:.5rem .75rem 0; border-bottom:1px solid var(--color-surface-300-700); }
  .collab-tabs button { position:relative; padding:.5rem .7rem; color:var(--color-surface-600-400); border-bottom:2px solid transparent; }
  .collab-tabs button.active { color:var(--color-primary-500); border-color:var(--color-primary-500); }
  .unread { display:inline-block; width:.45rem; height:.45rem; margin-left:.35rem; vertical-align:middle; border-radius:50%; background:var(--color-primary-500); }
</style>
