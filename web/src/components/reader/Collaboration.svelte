<script>
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import Chat from "../Chat.svelte";
  import Comments from "../Comments.svelte";

  let {
    messages = [], connected = false, canPost = false, onsend,
    comments = [], identity = "", commentingAs = "Anonymous",
    canModerate = false, canComment = true, mode = "",
    went = {}, replacements = {}, ontool, onreveal, onresolve, ondelete,
    ondeletemany, onreply, onaccept, onreject, onrejectconfirmed, pending,
    unreadChat = false, selected = "", tab = $bindable("comments"),
    // The draft being written, which belongs to the Comments tab whatever it
    // will become: a suggestion is a comment until it is sent.
    composing = null, needsLogin = false, signInHref = "", oncommentsend, oncommentcancel,
  } = $props();
  const VIEWS = [
    { id: "chat", label: "Chat" },
    { id: "comments", label: "Comments" },
    { id: "highlights", label: "Highlights" },
  ];
  const common = () => ({ identity, commentingAs, canModerate, canComment, mode,
    went, replacements, ontool, onreveal, onresolve, ondelete, ondeletemany,
    onreply, onaccept, onreject, onrejectconfirmed, pending, selected });
</script>

<!-- The tab strip is Skeleton's, which is Zag's, as the agent panel's is: the
     roving tabindex, the arrow-key wrap, Home and End, and the tab/panel
     pairing come from there rather than from a keydown handler written out
     again here. The ids are named so the panels keep the names the rest of
     the reader knows them by. -->
<section class="collaboration panel" aria-label="Collaboration">
  <Tabs class="collab-tabs-root" value={tab} onValueChange={({ value }) => (tab = value)}
        ids={{ trigger: (value) => `collaboration-tab-${value}`, content: (value) => `collaboration-panel-${value}` }}>
    <Tabs.List class="panel-tabs collab-tabs" aria-label="Collaboration views">
      {#each VIEWS as view (view.id)}
        <Tabs.Trigger class="collab-tab" value={view.id} title={view.label}>
          {view.label}{#if view.id === "chat" && unreadChat}<span class="unread" aria-label="Unread messages"></span>{/if}
        </Tabs.Trigger>
      {/each}
    </Tabs.List>
    <Tabs.Content value="chat" class="collab-panel"><Chat {messages} {connected} {canPost} {onsend} /></Tabs.Content>
    <Tabs.Content value="comments" class="collab-panel">
      <Comments {...common()} {comments} filter="comments" cardIdPrefix="collaboration-comment"
        {composing} {needsLogin} {signInHref} onsend={oncommentsend} oncancel={oncommentcancel} />
    </Tabs.Content>
    <Tabs.Content value="highlights" class="collab-panel">
      <Comments {...common()} {comments} filter="highlights" title="Highlights"
        cardIdPrefix="collaboration-highlight"
        emptyMessage="Select a passage and choose Highlight. Add a reply to discuss a highlight." />
    </Tabs.Content>
  </Tabs>
</section>

<style>
  .collaboration { display:flex; min-height:0; flex-direction:column; overflow:hidden; padding:0; }
  /* The tab root is Skeleton's element, so the column's height has to pass
     through it: without this the panels measure against the section and the
     comment list scrolls the whole reader instead of itself. */
  .collaboration :global(.collab-tabs-root) { display:flex; flex:1 1 auto; min-height:0; flex-direction:column; overflow:hidden; }
  .collaboration :global(.collab-panel) { display:flex; flex:1 1 auto; min-height:0; flex-direction:column; overflow:hidden; }
  .collaboration :global(.collab-panel[hidden]) { display:none; }
  /* Layout, truncation and the narrow-panel step live on .panel-tabs; the
     gutters stay there too, so this row and the agent panel's row compress at
     the same width instead of a gutter's worth apart. */
  .collaboration :global(.collab-tab) { color:var(--color-surface-600-400); border-bottom:2px solid transparent; }
  .collaboration :global(.collab-tab[data-selected]) { color:var(--color-primary-500); border-color:var(--color-primary-500); }
  .unread { display:inline-block; width:.45rem; height:.45rem; margin-left:.35rem; vertical-align:middle; border-radius:50%; background:var(--color-primary-500); }
</style>
