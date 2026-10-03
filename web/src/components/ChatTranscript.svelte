<script>
  import { tick } from "svelte";
  import Avatar from "./Avatar.svelte";
  // One transcript for every conversation the sidebar holds: the live chat
  // between people and the exchange with an agent render through this same
  // widget. Messages are grouped into series by author; the first post of a
  // series carries the avatar and the name, the rest sit in its gutter.
  let {
    messages = [],
    empty = "No messages yet.",
    label = "Conversation",
    roleLabel = (message) => message.creator || message.role,
    authorKey = (message) => `${message.role || ""}:${message.creator || ""}`,
    onresult = undefined,
    authors = true,
    quiet = false,
    after = undefined,
  } = $props();
  let transcript = $state();
  let following = true;
  let unread = $state(false);
  let fingerprint = "";

  const rows = $derived(
    messages.map((message, index) => {
      const key = authorKey(message);
      const starts = index === 0 || authorKey(messages[index - 1]) !== key;
      return { message, key, starts, name: roleLabel(message) };
    }),
  );

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

<div class="chat-transcript-wrap" class:quiet>
  <div class="chat-transcript" bind:this={transcript} onscroll={scrolled} role="log" aria-label={label} aria-live="polite" aria-relevant="additions text">
    {#each rows as { message, key, starts, name } (`${message.role || "chat"}:${message.id}`)}
      {@const results = message.context?.results}
      {@const refusedCount = results?.refused || 0}
      {@const unresolvedCount = results?.unresolved || 0}
      {@const confirmed = results?.confirmed || []}
      {@const confirmedOmitted = results?.confirmed_omitted || 0}
      <article class="chat-message" class:starts class:from-user={message.role === "user"} class:from-agent={message.role === "agent"} data-id={message.id}
        aria-label={starts && !authors ? name : undefined}>
        <span class="chat-gutter">
          {#if starts}<Avatar {name} {key} size={quiet ? 5 : 7} icon={message.role === "agent" ? "bot" : ""} />{/if}
        </span>
        <div class="chat-bubble">
          {#if starts}
            {#if authors}<strong class="chat-author">{name}</strong>{:else}<span class="sr-only">{name}</span>{/if}
          {/if}
          <p>{message.text}</p>
          {#each confirmed.filter(effect => effect.kind === "application" || effect.kind === "comment") as effect}
            <p>{effect.kind === "application" ? "Source changes applied." : `Comment action confirmed: ${effect.action}.`}</p>
          {/each}
          {#if confirmedOmitted}
            <p>{confirmedOmitted} additional confirmed effects omitted from this summary.</p>
          {/if}
          {#if refusedCount || unresolvedCount}
            <p class="effect-warning" role="alert">
              {#if refusedCount}{refusedCount} document operation{refusedCount === 1 ? " was" : "s were"} refused.{/if}
              {#if unresolvedCount} {unresolvedCount} document operation{unresolvedCount === 1 ? " has" : "s have"} an unconfirmed outcome.{/if}
            </p>
          {/if}
          {#if message.context?.results?.suggestions?.length}
            <button class="btn btn-sm lp-control-outline" onclick={() => onresult?.(message.context.results)}>Review changes</button>
          {/if}
        </div>
      </article>
    {/each}
    {#if !messages.length}<p class="panel-muted">{empty}</p>{/if}
    {@render after?.()}
  </div>
  {#if unread}<button class="btn btn-sm lp-control-outline new-messages" onclick={() => { following = true; void follow(); }}>New messages ↓</button>{/if}
</div>

<style>
  .chat-transcript-wrap { position: relative; display: flex; min-height: 0; flex: 1 1 0; }
  .chat-transcript { display: flex; min-height: 0; min-width: 0; flex: 1 1 0; flex-direction: column; gap: calc(var(--spacing) * 0.75); overflow-y: auto; overscroll-behavior: contain; padding-inline: 1px; }
  .chat-message { display: grid; grid-template-columns: calc(var(--spacing) * 7) minmax(0, 1fr); column-gap: calc(var(--spacing) * 2); align-items: start; min-width: 0; flex-shrink: 0; }
  /* A new speaker opens a new series: a little more air above it, and the avatar. */
  .chat-message.starts:not(:first-child) { margin-top: calc(var(--spacing) * 2.25); }
  .chat-gutter { display: block; width: calc(var(--spacing) * 7); height: calc(var(--spacing) * 7); }
  .chat-bubble { width: fit-content; max-width: 100%; min-width: 0; padding: calc(var(--spacing) * 1.25) calc(var(--spacing) * 2.5); border: 1px solid var(--color-border); border-radius: calc(var(--radius-base) * 2); background: var(--color-subtle); }
  .chat-message.starts .chat-bubble { border-top-left-radius: calc(var(--radius-base) * 0.5); }
  .chat-message.from-user .chat-bubble { border-color: color-mix(in srgb, var(--color-brand) 35%, var(--color-border)); background: color-mix(in srgb, var(--color-brand) 10%, var(--color-subtle)); }
  .chat-author { display: block; margin-bottom: calc(var(--spacing) * 0.5); font-size: var(--panel-meta-size); line-height: var(--panel-line-height); color: var(--panel-muted); }
  .chat-bubble p { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0; }
  .chat-bubble .effect-warning { margin-top: var(--spacing); color: var(--color-warning-text); font-weight: 600; }
  .chat-bubble .btn { margin-top: var(--spacing); }
  .new-messages { position: absolute; right: var(--spacing); bottom: var(--spacing); }

  /* Quiet variant: a tighter gutter, less padding, and no card behind agent
     text so the conversation reads as plain text flowing in the column. */
  .chat-transcript-wrap.quiet .chat-transcript { gap: calc(var(--spacing) * 0.5); }
  .chat-transcript-wrap.quiet .chat-message { grid-template-columns: calc(var(--spacing) * 5) minmax(0, 1fr); column-gap: var(--spacing); }
  .chat-transcript-wrap.quiet .chat-message.starts:not(:first-child) { margin-top: var(--spacing); }
  .chat-transcript-wrap.quiet .chat-gutter { width: calc(var(--spacing) * 5); height: calc(var(--spacing) * 5); }
  .chat-transcript-wrap.quiet .chat-bubble { padding: calc(var(--spacing) * 0.5) calc(var(--spacing) * 0.75); border: none; border-radius: var(--radius-base); background: none; }
  .chat-transcript-wrap.quiet .chat-message.starts .chat-bubble { border-top-left-radius: var(--radius-base); }
  .chat-transcript-wrap.quiet .chat-message.from-user .chat-bubble { background: color-mix(in srgb, var(--color-brand) 10%, var(--color-subtle)); }
</style>
