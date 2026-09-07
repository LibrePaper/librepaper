<script>
  import { onMount } from "svelte";
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

  $effect(() => {
    if (selection?.exact) selected = { ...selection };
  });

  onMount(() => {
    client = createAgentClient({ origin: location.origin, slug, link,
      onChange: (next) => { connection = next; } });
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
    } catch { problem = "Copy is unavailable; select the instructions and copy them manually."; }
  }

  async function newConversation() {
    await act(async () => {
      await client.end();
      await client.create();
    });
  }
</script>

<section class="panel agent-panel" aria-label="Agent chat">
  <PanelHeader title="Agent" meta={connection.listening ? "Listening" : "Waiting"} />
  <p class="panel-muted">Give the mailbox instructions to any agent that can run the Komodoc CLI. The conversation is stored privately; only holders of this conversation credential and document access can read it.</p>

  {#if connection.id}
    <div class="agent-actions">
      <span class="panel-meta" role="status">{connection.listening ? "Listening" : connection.connected ? "Waiting" : "Offline"}</span>
      <button class="btn btn-sm preset-tonal-surface" disabled={busy} onclick={() => void newConversation()}>New conversation</button>
    </div>

    <details class="agent-instructions">
      <summary class="panel-meta">Connect your agent</summary>
      <label class="label">Document link for the agent
        <input class="input" type="url" bind:value={agentLink} aria-describedby="agent-link-help" />
      </label>
      <p id="agent-link-help" class="panel-meta">Use a link from Share when this page has no share key. The link controls the agent’s document permissions.</p>
      {#if !validDocumentLink()}<p class="panel-muted" role="alert">Choose a link to this same document and server.</p>{/if}
      <label class="label">Conversation ID
        <input class="input" value={connection.id} readonly aria-label="Conversation ID" />
      </label>
      <label class="label">Conversation token
        <input class="input" value={connection.token} readonly aria-label="Conversation token" />
      </label>
      <label class="label">CLI instructions
        <textarea class="textarea" rows="6" readonly value={instructions()} aria-label="Copyable CLI instructions"></textarea>
      </label>
      <button class="btn btn-sm preset-tonal-surface" disabled={busy || !validDocumentLink()} onclick={() => void copyInstructions()}>{copied ? "Copied" : "Copy instructions"}</button>
    </details>

    <div class="agent-transcript" role="log" aria-label="Conversation" aria-live="polite" aria-relevant="additions text">
      {#each connection.messages as message (message.id)}
        <article class="agent-message">
          <strong class="panel-meta">{message.role === "user" ? "You" : "Agent"}</strong>
          <p>{message.text}</p>
        </article>
      {/each}
      {#if !connection.messages.length}<p class="panel-muted">Waiting for your agent. You can send a message while it is offline.</p>{/if}
    </div>

    <form class="agent-form" onsubmit={send}>
      {#if path}<p class="panel-meta">Current file: {path}</p>{/if}
      {#if selected}
        <label class="flex gap-2 items-center panel-meta"><input class="checkbox" type="checkbox" bind:checked={includeSelection} />Include selected passage</label>
        {#if includeSelection}<blockquote class="panel-muted agent-selection">{selected.exact}</blockquote>{/if}
      {/if}
      <label class="label">Message
        <textarea class="textarea" rows="4" bind:value={draft} placeholder="Ask your agent…" disabled={busy} required></textarea>
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
  .agent-panel { display: flex; flex-direction: column; gap: calc(var(--spacing) * 3); }
  .agent-form, .agent-instructions { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .agent-actions { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); }
  .agent-transcript { display: flex; flex-direction: column; gap: calc(var(--spacing) * 4); }
  .agent-message p { white-space: pre-wrap; overflow-wrap: anywhere; margin-top: var(--spacing); }
  .agent-selection { max-height: calc(var(--spacing) * 24); overflow: auto; }
  .agent-instructions textarea { font-family: var(--font-mono, monospace); font-size: 0.8rem; }
</style>
