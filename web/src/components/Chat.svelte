<script>
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  let { messages = [], connected = false, canPost = false, onsend, more = false, loadingOlder = false, loadError = "", onloadolder = undefined } = $props();
</script>
<section class="live-chat" aria-label="Chat">
  <h2 class="sr-only">Chat</h2>
  {#if !connected}<p class="chat-status panel-meta" role="status">Offline. Chat reconnects on its own.</p>{/if}
  <div class="chat-body">
    <ChatTranscript {messages} {more} {loadingOlder} {loadError} {onloadolder}
      emptyTitle="No messages yet" emptyText="Messages are shared with everyone who can comment on this project."
      roleLabel={(message) => message.creator || "Anonymous"} />
  </div>
  {#if !canPost}<p class="panel-muted">Comment access is required to chat.</p>{/if}
  <ChatComposer placeholder="Message everyone…" disabled={!connected || !canPost} {onsend} />
</section>
<style>
  /* The pane the collaboration panel puts this in carries the padding. */
  .live-chat { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 2); overflow:hidden; }
  .live-chat > :global(*) { flex-shrink:0; }
  .chat-body { display:flex; flex:1 1 0; min-height:0; flex-direction:column; }
  .chat-body > :global(*) { flex:1 1 0; min-height:0; }
</style>
