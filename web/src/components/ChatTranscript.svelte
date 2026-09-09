<script>
  import { tick } from "svelte";
  let { messages = [], empty = "No messages yet.", label = "Conversation", roleLabel = (message) => message.creator || message.role, onresult } = $props();
  let transcript = $state();
  let following = true;
  let unread = $state(false);
  let fingerprint = "";

  async function follow() {
    await tick();
    if (following && transcript?.clientHeight) transcript.scrollTop = transcript.scrollHeight;
    if (following) unread = false;
  }
  function scrolled() {
    if (!transcript?.clientHeight) return;
    following = transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight <= 24;
    if (following) unread = false;
  }
  $effect(() => {
    const last = messages.at(-1);
    const next = last ? `${last.id}:${last.text}` : "";
    if (next && next !== fingerprint) {
      if (following) void follow(); else unread = true;
    }
    fingerprint = next;
  });
  $effect(() => {
    const element = transcript;
    if (!element) return;
    const observer = new ResizeObserver(() => { if (following) void follow(); });
    observer.observe(element);
    return () => observer.disconnect();
  });
</script>

<div class="chat-transcript-wrap">
  <div class="chat-transcript" bind:this={transcript} onscroll={scrolled} role="log" aria-label={label} aria-live="polite" aria-relevant="additions text">
    {#each messages as message (`${message.role || "chat"}:${message.id}`)}
      <article class="chat-message" data-id={message.id}>
        <strong class="panel-meta">{roleLabel(message)}</strong>
        <p>{message.text}</p>
        {#if message.context?.results && (message.context.results.suggestions?.length || message.context.results.pass)}
          <button class="btn btn-sm preset-tonal-surface" onclick={() => onresult?.(message.context.results)}>Review changes</button>
        {/if}
      </article>
    {/each}
    {#if !messages.length}<p class="panel-muted">{empty}</p>{/if}
  </div>
  {#if unread}<button class="btn btn-sm preset-tonal-surface new-messages" onclick={() => { following = true; void follow(); }}>New messages ↓</button>{/if}
</div>

<style>
  .chat-transcript-wrap { position: relative; display: flex; min-height: 0; flex: 1 1 0; }
  .chat-transcript { display: flex; min-height: 0; min-width: 0; flex: 1 1 0; flex-direction: column; gap: calc(var(--spacing) * 4); overflow-y: auto; overscroll-behavior: contain; }
  .chat-message p { white-space: pre-wrap; overflow-wrap: anywhere; margin-top: var(--spacing); }
  .new-messages { position: absolute; right: var(--spacing); bottom: var(--spacing); }
</style>
