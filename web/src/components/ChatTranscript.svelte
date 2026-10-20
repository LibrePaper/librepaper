<script>
  import { tick } from "svelte";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";
  import { chatRows } from "../lib/chat-rows.js";
  // One transcript for every conversation the sidebar holds: the live chat
  // between people and the exchange with an agent render through this same
  // widget. A compact, bubble-free layout in the style of a chat app: a day
  // divider where the date changes, and a series of posts by one author under
  // a single header with the avatar. Later posts in the series show their time
  // in the gutter when hovered.
  let {
    messages = [],
    emptyTitle = "No messages yet",
    emptyText = "",
    label = "Conversation",
    roleLabel = (message) => message.creator || message.role,
    authorKey = (message) => `${message.role || ""}:${message.creator || ""}`,
    onresult = undefined,
    after = undefined,
    // Older messages the server still holds, for a transcript that is a
    // window onto a longer history.
    more = false,
    loadingOlder = false,
    loadError = "",
    onloadolder = undefined,
  } = $props();
  let transcript = $state();
  let following = true;
  let unread = $state(false);
  let fingerprint = "";

  const rows = $derived(chatRows(messages, { authorKey, roleLabel }));

  async function follow() {
    await tick();
    if (following && transcript?.clientHeight) transcript.scrollTop = transcript.scrollHeight;
    if (following) unread = false;
  }
  // Prepending moves everything down by the height it adds, so the scroll
  // position is moved down by the same amount and the reader stays put.
  async function older() {
    if (!transcript || !more || loadingOlder || !onloadolder) return;
    const before = transcript.scrollHeight;
    const top = transcript.scrollTop;
    await onloadolder();
    await tick();
    if (transcript) transcript.scrollTop = top + (transcript.scrollHeight - before);
  }
  function scrolled() {
    if (!transcript?.clientHeight) return;
    if (transcript.scrollTop <= 8) void older();
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
    {#if more && onloadolder}
      <button type="button" class="btn btn-sm lp-control-tonal-brand older" disabled={loadingOlder} onclick={older}>
        {loadingOlder ? "Loading..." : "Load older messages"}
      </button>
    {/if}
    {#if loadError}<p class="panel-muted" role="alert">{loadError}</p>{/if}
    {#each rows as { message, key, starts, name, date, time, stamp } (`${message.role || "chat"}:${message.id}`)}
      {@const results = message.context?.results}
      {@const refusedCount = results?.refused || 0}
      {@const unresolvedCount = results?.unresolved || 0}
      {@const confirmed = results?.confirmed || []}
      {@const confirmedOmitted = results?.confirmed_omitted || 0}
      {#if date}
        <div class="chat-date" role="separator"><span>{date}</span></div>
      {/if}
      <article class="chat-message" class:starts data-id={message.id}>
        <span class="chat-gutter">
          {#if starts}
            <Avatar {name} {key} size={7} icon={message.role === "agent" ? "bot" : ""} />
          {:else if time}
            <time class="chat-time chat-time-hover" datetime={message.created} title={stamp}>{time}</time>
          {/if}
        </span>
        <div class="chat-body">
          {#if starts}
            <div class="chat-meta">
              <strong class="chat-author">{name}</strong>
              {#if time}<time class="chat-time" datetime={message.created} title={stamp}>{time}</time>{/if}
            </div>
          {/if}
          <p class="chat-text" title={starts ? undefined : stamp}>{message.text}</p>
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
          {#if message.context?.results?.changes?.length}
            <button class="btn btn-sm lp-control-tonal-brand" onclick={() => onresult?.(message.context.results)}>Review changes</button>
          {/if}
        </div>
      </article>
    {/each}
    {#if !messages.length && !loadError}
      <div class="chat-empty">
        <span class="chat-empty-icon"><Icon name="message-square" /></span>
        <p class="chat-empty-title">{emptyTitle}</p>
        {#if emptyText}<p class="chat-empty-text">{emptyText}</p>{/if}
      </div>
    {/if}
    {@render after?.()}
  </div>
  {#if unread}
    <button class="btn btn-sm lp-control-tonal-brand new-messages" onclick={() => { following = true; void follow(); }}>New messages <Icon name="arrow-down" /></button>
  {/if}
</div>

<style>
  .chat-transcript-wrap { position: relative; display: flex; min-width: 0; min-height: 0; flex: 1 1 0; }
  .chat-transcript { display: flex; min-width: 0; min-height: 0; flex: 1 1 0; flex-direction: column; gap: calc(var(--spacing) * 0.5); overflow-x: hidden; overflow-y: auto; overscroll-behavior: contain; padding-inline: 1px; }
  .chat-date { display: flex; flex-shrink: 0; align-items: center; gap: calc(var(--spacing) * 2); margin-block: calc(var(--spacing) * 2); color: var(--panel-muted); font-size: var(--panel-meta-size); line-height: var(--panel-line-height); }
  .chat-date::before, .chat-date::after { content: ""; flex: 1 1 0; height: 1px; background: var(--color-border); }
  .chat-date span { flex-shrink: 0; }
  .chat-message { display: grid; grid-template-columns: calc(var(--spacing) * 7) minmax(0, 1fr); column-gap: calc(var(--spacing) * 2); align-items: start; min-width: 0; flex-shrink: 0; }
  /* A new author or a long pause opens a new group: a little more air above it. */
  .chat-message.starts:not(:first-child) { margin-top: calc(var(--spacing) * 3); }
  .chat-gutter { display: block; width: calc(var(--spacing) * 7); min-width: 0; height: calc(var(--spacing) * 7); }
  .chat-body { display: flex; min-width: 0; flex-direction: column; align-items: flex-start; }
  .chat-meta { display: flex; min-width: 0; max-width: 100%; align-items: baseline; gap: calc(var(--spacing) * 2); line-height: var(--panel-line-height); }
  .chat-author { min-width: 0; overflow: hidden; color: var(--panel-text, inherit); font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
  .chat-time { flex-shrink: 0; color: var(--panel-muted); font-size: var(--panel-meta-size); line-height: var(--panel-line-height); }
  .chat-time-hover { display: block; height: calc(var(--spacing) * 7); overflow: hidden; color: var(--panel-muted); font-size: var(--panel-meta-size); line-height: calc(var(--spacing) * 7); text-align: center; opacity: 0; }
  .chat-message:hover .chat-time-hover, .chat-message:focus-within .chat-time-hover { opacity: 1; }
  .chat-text, .chat-body p { min-width: 0; margin: 0; max-width: 100%; white-space: pre-wrap; overflow-wrap: anywhere; }
  .chat-body .effect-warning { margin-top: var(--spacing); color: var(--color-warning-text); font-weight: 600; }
  .chat-body .btn { margin-top: var(--spacing); }
  .chat-empty { display: flex; min-width: 0; max-width: 100%; flex-direction: column; align-items: center; gap: calc(var(--spacing) * 1.5); margin: auto; padding-block: calc(var(--spacing) * 4); text-align: center; }
  .chat-empty-icon { display: inline-flex; color: var(--panel-muted); }
  .chat-empty-icon :global(svg) { width: 20px; height: 20px; }
  .chat-empty-title { margin: 0; color: var(--panel-text, inherit); }
  .chat-empty-text { margin: 0; color: var(--panel-muted); font-size: var(--panel-meta-size); line-height: var(--panel-line-height); }
  .older { align-self: center; flex-shrink: 0; }
  .new-messages { position: absolute; bottom: var(--spacing); left: 50%; transform: translateX(-50%); display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5); }
  @media (hover: none) {
    .chat-time-hover { display: none; }
  }
</style>
