<script>
  import PanelHeader from "./PanelHeader.svelte";
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  let { messages = [], connected = false, canPost = false, onsend } = $props();
</script>
<section class="panel live-chat" aria-label="Chat">
  <PanelHeader title="Chat" meta={connected ? "Live" : "Offline"} />
  <ChatTranscript {messages} empty="No live messages yet." roleLabel={(message) => message.creator || "Anonymous"} />
  <ChatComposer placeholder="Message everyone here…" disabled={!connected || !canPost} {onsend} />
  {#if !canPost}<p class="panel-muted">Comment access is required to chat.</p>{/if}
</section>
<style>
  .live-chat { display:flex; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); overflow:hidden; }
  .live-chat > :global(*) { flex-shrink:0; }
</style>
