<script>
  import { onMount } from "svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  import { createAgentClient } from "../lib/agent-client.js";

  let { slug, link, path = "", selection = null } = $props();
  let client;
  let connection = $state({ id: "", token: "", messages: [],
    connected: false, listening: false, error: "" });
  let busy = $state(false);
  let problem = $state("");
  let includeSelection = $state(true);
  let selected = $state(null);
  let agentLink = $state("");
  let copied = $state(false);
  let agentSeen = $state(false);

  $effect(() => {
    if (selection?.exact) selected = { ...selection };
  });

  onMount(() => {
    client = createAgentClient({ origin: location.origin, slug, link,
      onChange: (next) => {
        connection = next;
        if (next.listening || next.messages.some((message) => message.role === "agent")) agentSeen = true;
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

  async function send(text) {
    if (busy || !connection.listening) return false;
    let sent = false;
    await act(async () => {
      await client.send(text, { file: path,
        ...(includeSelection && selected ? { selection: selected } : {}) });
      sent = true;
    });
    return sent;
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
      "Install the Komodoc skills: npx skills add vincentarelbundock/komodoc",
      "Give these commands to your preferred agent. Run watch whenever the agent is ready for your next message; messages are never queued.",
      `komodoc agent chat watch ${shell(documentLink)} --conversation ${shell(connection.id)} --token ${shell(connection.token)} --timeout 25`,
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
      <p class="panel-muted">Copy the connection instructions to an agent that can run the Komodoc CLI. This live conversation is private and is not saved.</p>
      <label class="label">Document link for the agent
        <input class="input" type="url" bind:value={agentLink} aria-describedby="agent-link-help" />
      </label>
      <p id="agent-link-help" class="panel-meta">Use a link from Share when this page has no share key. The link controls the agent’s document permissions.</p>
      {#if !validDocumentLink()}<p class="panel-muted" role="alert">Choose a link to this same document and server.</p>{/if}
      </div>
    {/if}

    <ChatTranscript messages={connection.messages} empty={connection.listening ? "No live messages yet." : "Connect your agent to start chatting."} roleLabel={(message) => message.role === "user" ? "You" : "Agent"} />

    <div class="agent-context">
      {#if path}<p class="panel-meta">Current file: {path}</p>{/if}
      {#if selected}
        <label class="flex gap-2 items-center panel-meta"><input class="checkbox" type="checkbox" bind:checked={includeSelection} />Include selected passage</label>
        {#if includeSelection}<blockquote class="panel-muted agent-selection">{selected.exact}</blockquote>{/if}
      {/if}
    </div>
    <ChatComposer placeholder="Ask your agent…" disabled={busy || !connection.listening} onsend={send} />
  {:else}
    <button class="btn preset-filled-primary-500" disabled={busy} onclick={() => void act(() => client.create())}>Start conversation</button>
    <p class="panel-muted">A private live channel will be created. Messages are not stored, so the agent must remain connected.</p>
  {/if}
  {#if problem || connection.error}<p class="panel-muted" role="alert">{problem || connection.error}</p>{/if}
</section>

<style>
  .agent-panel { display: flex; min-height: 0; flex-direction: column; gap: calc(var(--spacing) * 3); overflow: hidden; }
  .agent-panel > :global(*) { flex-shrink: 0; }
  .agent-context, .agent-setup { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .agent-actions { display: flex; align-items: center; justify-content: space-between; gap: calc(var(--spacing) * 2); }
  .agent-action-buttons { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--spacing); }
  .agent-selection { max-height: calc(var(--spacing) * 24); overflow: auto; }
</style>
