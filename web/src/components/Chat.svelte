<script>
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  import { m } from "../lib/paraglide/messages.js";
  let { messages = [], connected = false, canPost = false, onsend, more = false, loadingOlder = false, loadError = "", onloadolder = undefined } = $props();
</script>
<section class="live-chat" aria-label={m.common_chat()}>
  <h2 class="sr-only">{m.common_chat()}</h2>
  {#if !connected}<p class="chat-status panel-meta" role="status">{m.chat_offline()}</p>{/if}
  <div class="chat-body">
    <ChatTranscript {messages} {more} {loadingOlder} {loadError} {onloadolder}
      emptyTitle={m.common_no_messages_yet()} emptyText={m.chat_empty_text()}
      roleLabel={(message) => message.creator || m.common_anonymous()} />
  </div>
  {#if !canPost}<p class="panel-muted">{m.chat_comment_required()}</p>{/if}
  <ChatComposer placeholder={m.chat_placeholder()} disabled={!connected || !canPost} {onsend} />
</section>
<style>
  /* The pane the collaboration panel puts this in carries the padding. */
  .live-chat { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 2); overflow:hidden; }
  .live-chat > :global(*) { flex-shrink:0; }
  .chat-body { display:flex; flex:1 1 0; min-height:0; flex-direction:column; }
  .chat-body > :global(*) { flex:1 1 0; min-height:0; }
</style>
