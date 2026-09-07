<script>
  import { onMount, tick } from "svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import { createAgentClient } from "../lib/agent-client.js";

  let { slug, link, path = "", selection = null } = $props();
  let client;
  let connection = $state({ id: "", token: "", cursor: 0, messages: [],
    connected: false, listening: false, error: "" });
  let draft = $state("");
  let busy = $state(false);
  let problem = $state("");
  let includeSelection = $state(true);
  let selected = $state(null);
  let agentLink = $state("");
  let copied = $state(false);
  let transcript = $state();
  let messageFingerprint = $state("");
  let agentSeen = $state(false);
  let unreadMessages = $state(false);
  let following = true;
  let scrollTop = 0;

  function isAtBottom(element = transcript) {
    return !element || element.scrollHeight - element.scrollTop - element.clientHeight <= 24;
  }

  async function followTranscript() {
    await tick();
    if (!following || !transcript?.clientHeight) return;
    transcript.scrollTop = transcript.scrollHeight;
    scrollTop = transcript.scrollTop;
    unreadMessages = false;
  }

  function transcriptScrolled() {
    if (!transcript?.clientHeight) return;
    scrollTop = transcript.scrollTop;
    following = isAtBottom();
    if (following) unreadMessages = false;
  }

  function showLatest() {
    following = true;
    void followTranscript();
  }

  $effect(() => {
    const element = transcript;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      if (!element.clientHeight) return;
      if (following) void followTranscript();
      else element.scrollTop = scrollTop;
    });
    observer.observe(element);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (selection?.exact) selected = { ...selection };
  });

  onMount(() => {
    client = createAgentClient({ origin: location.origin, slug, link,
      onChange: (next) => {
        const follow = following;
        const lastMessage = next.messages.at(-1);
        const fingerprint = lastMessage ? `${lastMessage.id}:${lastMessage.text}` : "";
        const changed = fingerprint !== messageFingerprint;
        const retained = new Set(next.messages.map((message) => message.id));
        const anchor = changed && !follow && transcript?.clientHeight
          ? [...transcript.querySelectorAll('.agent-message')]
            .map((element) => ({ element, top: element.getBoundingClientRect().top }))
            .find(({ element }) => retained.has(element.dataset.id) && element.getBoundingClientRect().bottom > transcript.getBoundingClientRect().top)
          : null;
        connection = next;
        messageFingerprint = fingerprint;
        if (next.listening || next.messages.some((message) => message.role === "agent")) agentSeen = true;
        if (changed) {
          if (follow) void followTranscript();
          else {
            unreadMessages = true;
            if (anchor) void tick().then(() => {
              const top = anchor.element.getBoundingClientRect().top;
              if (transcript?.clientHeight && anchor.element.isConnected) {
                transcript.scrollTop += top - anchor.top;
                scrollTop = transcript.scrollTop;
              }
            });
          }
        }
      } });
    agentLink = link || "";
    connection = client.current;
    void (async () => {
      await client.resume();
      if (!client.current.id) {
        try { await client.create(); }
        catch (error) { problem = error.message; }
      }
    })();
    return () => client.dispose();
  });

  async function act(operation) {
    busy = true; problem = "";
    try { await operation(); }
    catch (error) { problem = error.message; }
    finally { busy = false; }
  }

  async function send(event) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || busy) return;
    await act(async () => {
      await client.send(text, { file: path,
        ...(includeSelection && selected ? { selection: selected } : {}) });
      draft = "";
    });
  }

  function messageKeydown(event) {
    if (event.key !== "Enter" || event.isComposing || event.keyCode === 229) return;
    if (event.ctrlKey) {
      event.preventDefault();
      const textarea = event.currentTarget;
      const start = textarea.selectionStart;
      const end = textarea.selectionEnd;
      draft = `${draft.slice(0, start)}\n${draft.slice(end)}`;
      void tick().then(() => textarea.setSelectionRange(start + 1, start + 1));
      return;
    }
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  }

  function shell(value) {
    return `'${String(value).replaceAll("'", "'\\''")}'`;
  }

  function validDocumentLink() {
    try {
      const chosen = new URL(agentLink || link, location.href);
      const page = new URL(link, location.href);
      return ["http:", "https:"].includes(chosen.protocol)
        && !chosen.username && !chosen.password
        && chosen.origin === page.origin && chosen.pathname === page.pathname;
    } catch { return false; }
  }

  function instructions() {
    if (!connection.id || !connection.token || !validDocumentLink()) return "";
    const documentLink = agentLink || link;
    return [
      "Install the Komodoc skill: https://raw.githubusercontent.com/vincentarelbundock/komodoc/main/skills/komodoc/SKILL.md",
      "Give these commands to your preferred agent. Repeat watch with the returned next_cursor as --after.",
      `komodoc agent chat watch ${shell(documentLink)} --conversation ${shell(connection.id)} --token ${shell(connection.token)} --after 0 --timeout 25`,
      `komodoc agent chat post ${shell(documentLink)} --conversation ${shell(connection.id)} --token ${shell(connection.token)} --message 'Your reply'`,
    ].join("\n");
  }

  async function copyInstructions() {
    try {
      await navigator.clipboard.writeText(instructions());
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch { problem = "Copy is unavailable in this browser."; }
  }

  async function newConversation() {
    await act(async () => {
      await client.end();
      agentSeen = false;
      unreadMessages = false;
      following = true;
      await client.create();
    });
  }
</script>

<section class="panel agent-panel" aria-label="Agent chat">
  <PanelHeader title="Agent" meta={connection.listening ? "Listening" : "Waiting"} />

  {#if connection.id}
    <div class="agent-actions">
      <span class="panel-meta" role="status">{connection.listening ? "Listening" : connection.connected ? "Waiting" : "Offline"}</span>
      <div class="agent-action-buttons">
        <button class="btn btn-sm preset-tonal-surface" disabled={busy || !validDocumentLink()} onclick={() => void copyInstructions()}>{copied ? "Copied" : "Copy instructions"}</button>
        <button class="btn btn-sm preset-tonal-surface" disabled={busy} onclick={() => void newConversation()}>New conversation</button>
      </div>
    </div>

    {#if !agentSeen}
      <div class="agent-setup">
      <p class="panel-muted">Copy the connection instructions to an agent that can run the Komodoc CLI. The conversation is private to holders of its credential and document access.</p>
      <label class="label">Document link for the agent
        <input class="input" type="url" bind:value={agentLink} aria-describedby="agent-link-help" />
      </label>
      <p id="agent-link-help" class="panel-meta">Use a link from Share when this page has no share key. The link controls the agent’s document permissions.</p>
      {#if !validDocumentLink()}<p class="panel-muted" role="alert">Choose a link to this same document and server.</p>{/if}
      </div>
    {/if}

    <div class="agent-transcript-wrap">
    <div class="agent-transcript" bind:this={transcript} onscroll={transcriptScrolled} role="log" aria-label="Conversation" aria-live="polite" aria-relevant="additions text">
      {#each connection.messages as message (message.id)}
        <article class="agent-message" data-id={message.id}>
          <strong class="panel-meta">{message.role === "user" ? "You" : "Agent"}</strong>
          <p>{message.text}</p>
        </article>
      {/each}
      {#if !connection.messages.length}<p class="panel-muted">Waiting for your agent. You can send a message while it is offline.</p>{/if}
    </div>
    {#if unreadMessages}<button class="btn btn-sm preset-tonal-surface new-messages" onclick={showLatest}>New messages ↓</button>{/if}
    </div>

    <form class="agent-form" onsubmit={send}>
      {#if path}<p class="panel-meta">Current file: {path}</p>{/if}
      {#if selected}
        <label class="flex gap-2 items-center panel-meta"><input class="checkbox" type="checkbox" bind:checked={includeSelection} />Include selected passage</label>
        {#if includeSelection}<blockquote class="panel-muted agent-selection">{selected.exact}</blockquote>{/if}
      {/if}
      <label class="label">Message
        <textarea class="textarea" rows="4" bind:value={draft} placeholder="Ask your agent…" aria-label="Message" disabled={busy} required onkeydown={messageKeydown}></textarea>
      </label>
      <button class="btn preset-filled-primary-500" disabled={busy || !draft.trim()}>Send</button>
    </form>
  {:else}
    <button class="btn preset-filled-primary-500" disabled={busy} onclick={() => void act(() => client.create())}>Start conversation</button>
    <p class="panel-muted">A private mailbox will be created on this document. Then copy its CLI instructions to your preferred agent.</p>
  {/if}
  {#if problem || connection.error}<p class="panel-muted" role="alert">{problem || connection.error}</p>{/if}
</section>

<style>
  .agent-panel { display: flex; min-height: 0; flex-direction: column; gap: calc(var(--spacing) * 3); overflow: hidden; }
  .agent-panel > :global(*) { flex-shrink: 0; }
  .agent-form, .agent-setup { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .agent-actions { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); }
  .agent-action-buttons { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--spacing); }
  .agent-transcript-wrap { position: relative; display: flex; min-height: 0; flex: 1 1 0; }
  .agent-transcript { display: flex; min-height: 0; min-width: 0; flex: 1 1 0; flex-direction: column; gap: calc(var(--spacing) * 4); overflow-y: auto; overscroll-behavior: contain; }
  .new-messages { position: absolute; right: var(--spacing); bottom: var(--spacing); }
  .agent-message p { white-space: pre-wrap; overflow-wrap: anywhere; margin-top: var(--spacing); }
  .agent-selection { max-height: calc(var(--spacing) * 24); overflow: auto; }
</style>
