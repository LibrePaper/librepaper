<script>
  import { onMount } from "svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  import { createAgentClient } from "../lib/agent-client.js";
  import {
    captureAttachment, capabilityAllows,
    normalizeCapabilities, scopeLabel, visibleResults,
  } from "../lib/assistant.js";

  let {
    slug, link, path = "", selection = null, revision = "", request = null,
    comments = [], onreview,
  } = $props();
  let client;
  let connection = $state({ id: "", token: "", messages: [], connected: false,
    listening: false, agentJoined: false, ended: false, error: "", capabilities: null });
  let busy = $state(false);
  let starting = $state(true);
  let problem = $state("");
  let attachment = $state(null);
  let task = $state(null);
  let scope = $state("selection");
  let diagnostic = $state(null);
  let diagnosticRevision = $state("");
  let draft = $state("");
  let agentLink = $state("");
  let copied = $state(false);
  let setupOpen = $state(true);
  let resetPending = $state(false);
  let pendingRequest = $state(null);
  let lastRequestId = "";
  let lastSelection = $state(null);
  let suppressedSelection = $state("");
  let capabilityGeneration = 0;
  let verifiedCapabilities = $state(null);
  const selectedText = $derived(attachment?.selection?.exact || "");
  const caps = $derived(normalizeCapabilities(verifiedCapabilities));
  const contextPath = $derived(diagnostic?.file || diagnostic?.path || path);
  const preparedTaskValid = $derived(!task || capabilityAllows(caps, task, {
    attached: attachment?.anchored ? attachment : (task.kind === "explain" ? attachment : null), path: contextPath,
  }));
  const sendable = $derived(connection.listening && preparedTaskValid);
  const chipScope = (kind) => ["tighten", "rewrite"].includes(kind) ? "selection"
    : kind === "proofread" ? (scope === "document" ? "document" : "file")
      : scope === "selection" && !attachment ? (contextPath ? "file" : "document") : scope;
  const canPrepare = (kind) => capabilityAllows(caps, { kind, scope: chipScope(kind) }, {
    attached: attachment?.anchored ? attachment : (kind === "explain" ? attachment : null), path: contextPath,
  });
  const status = $derived(
    connection.ended ? "Connection ended"
      : !connection.id ? "Connect your agent"
        : connection.listening ? "Ready for your message"
          : connection.agentJoined ? "Agent isn't listening" : "Connect your agent",
  );
  const statusHelp = $derived(
    connection.ended ? "Reconnect to create a fresh setup prompt. Your draft will stay here."
      : connection.listening ? "Your agent is receiving messages."
        : connection.agentJoined ? "Resume watch in the agent's own window."
          : "Copy the setup prompt and run it in your agent's own window.",
  );
  const lastResult = $derived(connection.messages.reduce((found, message) =>
    message?.role === "agent" && message?.context?.results ? message : found, null));
  const reviewable = $derived(lastResult ? visibleResults(lastResult, comments) : { suggestions: [], pass: null });

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

  function selectionKey(value) {
    const source = value?.source || value;
    return value?.exact ? JSON.stringify([value.exact, source.path, source.prefix, source.suffix, source.position]) : "";
  }

  function instructions() {
    if (!connection.id || !connection.token || connection.ended || !validDocumentLink()) return "";
    const documentLink = agentLink || link;
    return [
      "Check the LibrePaper CLI: librepaper --version && librepaper agent --help. If missing or outdated, follow https://github.com/vincentarelbundock/librepaper#install before continuing.",
      "Load the pairing, document, and writing assistant skills: npx skills add vincentarelbundock/librepaper.",
      `export LIBREPAPER_DOCUMENT=${shell(documentLink)}`,
      `export LIBREPAPER_CHAT_TOKEN=${shell(connection.token)}`,
      `export LIBREPAPER_CONVERSATION=${shell(connection.id)}`,
      'librepaper agent capabilities "$LIBREPAPER_DOCUMENT"',
      `librepaper agent chat watch "$LIBREPAPER_DOCUMENT" --conversation ${shell(connection.id)} --timeout 25`,
      "Run watch again whenever you are ready for the next message. Use the document and writing skills for anchored suggestions and report result identifiers in context.results.",
    ].join("\n");
  }

  async function act(operation) {
    busy = true; problem = "";
    try { await operation(); }
    catch (error) { problem = error?.message || String(error); }
    finally { busy = false; }
  }

  function saveDraft(value) {
    draft = value;
  }

  function attach(value, capturedRevision = revision) {
    if (!value?.exact && !value?.source?.exact) return;
    const captured = captureAttachment(value, path, value.revision || capturedRevision);
    if (!captured) return;
    attachment = captured;
    lastSelection = value;
    suppressedSelection = "";
    if (!task || task.scope === "selection") scope = "selection";
  }

  function replaceSelection() {
    const current = selection?.exact ? selection : null;
    if (current) attach(current, current.revision || revision);
  }

  function removeSelection() {
    suppressedSelection = selectionKey(selection || lastSelection);
    attachment = null;
    if (scope === "selection") scope = path ? "file" : "document";
    if (task?.scope === "selection") task = null;
  }

  function applyRequest(next) {
    if (!next) return;
    pendingRequest = null;
    saveDraft("");
    task = null;
    diagnostic = next.diagnostic || null;
    diagnosticRevision = next.diagnostic ? next.revision || next.diagnostic.revision || "" : "";
    if (next.diagnostic) {
      suppressedSelection = selectionKey(selection || lastSelection);
      attachment = null;
    }
    if (next.selection) {
      attach(next.selection, next.revision || revision);
      scope = "selection";
    } else if (next.diagnostic) {
      scope = next.diagnostic.file || next.diagnostic.path ? "file" : "document";
    }
    if (next.diagnostic) {
      task = { kind: "explain", scope };
      if (!draft.trim()) saveDraft("Explain this diagnostic.");
    }
    lastRequestId = next.id || lastRequestId;
  }

  function requestArrived(next) {
    if (!next?.id || next.id === lastRequestId) return;
    lastRequestId = next.id;
    if (draft.trim()) pendingRequest = next;
    else applyRequest(next);
  }

  function chooseTask(kind) {
    const nextScope = chipScope(kind);
    if (!capabilityAllows(caps, { kind, scope: nextScope }, {
      attached: attachment?.anchored ? attachment : (kind === "explain" ? attachment : null), path: contextPath,
    })) return;
    const previous = task ? promptFor(task.kind, task.scope) : "";
    scope = nextScope;
    task = { kind, scope: nextScope };
    if (nextScope !== "selection") { suppressedSelection = selectionKey(selection || lastSelection); attachment = null; }
    const prompt = promptFor(kind, nextScope);
    saveDraft(!draft.trim() || draft === previous ? prompt : `${prompt}\n\n${draft}`);
    if (kind !== "explain") { diagnostic = null; diagnosticRevision = ""; }
  }

  function promptFor(kind, chosenScope) {
    const target = chosenScope === "document" ? "the whole document" : chosenScope === "file" ? "this file" : "the selected passage";
    return `${{ proofread: "Proofread", tighten: "Tighten", rewrite: "Rewrite", explain: "Explain" }[kind] || kind} ${target}.`;
  }

  function chooseScope(next) {
    const previous = task ? promptFor(task.kind, task.scope) : "";
    scope = next;
    if (next !== "selection") { suppressedSelection = selectionKey(selection || lastSelection); attachment = null; }
    if (task) {
      task = { ...task, scope: next };
      if (draft === previous) saveDraft(promptFor(task.kind, next));
    }
  }

  async function send(text) {
    if (busy || !sendable) return false;
    let sent = false;
    await act(async () => {
      await client.send(text, {
        task: task || undefined, attachment,
        path: task?.scope === "file" || !task ? contextPath : "",
        revision: diagnosticRevision || attachment?.revision || (task?.scope === "selection" ? revision : ""),
        diagnostic,
      });
      sent = true;
    });
    return sent;
  }

  async function copyInstructions() {
    try {
      await navigator.clipboard.writeText(instructions());
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch { problem = "Copy is unavailable in this browser."; }
  }

  async function reconnect() { await act(() => client.reconnect()); }

  async function newConversation() {
    if (!resetPending) { resetPending = true; return; }
    await act(async () => {
      await client.end();
      suppressedSelection = selectionKey(selection || lastSelection);
      saveDraft(""); task = null; attachment = null; diagnostic = null; diagnosticRevision = "";
      resetPending = false;
      await client.create();
    });
  }

  function chooseResult() {
    if (reviewable.suggestions.length || reviewable.pass) onreview?.(reviewable);
  }

  $effect(() => {
    if (!selection?.exact) return;
    const same = selectionKey(lastSelection) === selectionKey(selection);
    if (selectionKey(selection) === suppressedSelection) return;
    if (!attachment || (same && !attachment.revision && (selection.revision || revision))) {
      attach(selection, selection.revision || revision);
    }
  });
  $effect(() => requestArrived(request));
  async function checkCapabilities(nextLink = agentLink) {
    const generation = ++capabilityGeneration;
    verifiedCapabilities = null;
    client?.setLink(nextLink);
    try {
      const result = await client?.capabilities(nextLink);
      if (generation === capabilityGeneration) verifiedCapabilities = result;
    } catch {
      if (generation === capabilityGeneration) verifiedCapabilities = null;
    }
  }

  onMount(() => {
    agentLink = link || "";
    client = createAgentClient({ origin: location.origin, slug, link: agentLink || link,
      onChange: (next) => { connection = next; } });
    connection = client.current;
    void (async () => {
      await client.resume();
      await checkCapabilities(agentLink || link);
      if (!client.current.id) {
        try { await client.create(); }
        catch (error) { problem = error.message; }
      }
      starting = false;
    })();
    return () => client.dispose();
  });

</script>

<section class="panel agent-panel" aria-label="Agent chat">
  <PanelHeader title="Agent" />
  <div class="agent-status" role="status" aria-live="polite">
    <strong>{status}</strong><span class="panel-muted">{statusHelp}</span>
  </div>
  {#if !connection.id}<button class="btn preset-filled-primary-500" disabled={busy || starting} onclick={() => void act(() => client.create())}>{starting ? "Connecting…" : "Retry connection"}</button>{/if}

  {#if connection.ended}
    <div class="agent-recovery">
      <p class="panel-muted">This channel ended or was revoked. Reconnect for fresh credentials and setup instructions.</p>
      <button class="btn preset-filled-primary-500" disabled={busy} onclick={() => void reconnect()}>Reconnect agent</button>
    </div>
  {/if}

  <details class="agent-setup" open={setupOpen} ontoggle={(event) => setupOpen = event.currentTarget.open}>
    <summary>Connection settings</summary>
    <div class="setup-body">
      <p class="panel-muted">Connect an AI agent you already use. Copy the setup prompt into its own window; LibrePaper does not run a model or save this conversation.</p>
      <label class="label">Document link for the agent
        <input class="input" type="url" bind:value={agentLink} oninput={(event) => void checkCapabilities(event.currentTarget.value)} aria-describedby="agent-link-help" />
      </label>
      <p id="agent-link-help" class="panel-meta">The link determines what the agent may do. The browser checks the effective access for this link.</p>
      <div class="setup-actions">
        <button class="btn btn-sm preset-filled-primary-500" disabled={busy || connection.ended || !connection.id || !validDocumentLink()} onclick={() => void copyInstructions()}>{copied ? "Copied" : "Copy setup prompt"}</button>
        {#if connection.id}<button class="btn btn-sm preset-tonal-surface" disabled={busy} onclick={() => void act(() => checkCapabilities(agentLink))}>Check access</button>{/if}
      </div>
      {#if !validDocumentLink()}<p class="panel-muted" role="alert">Choose a link to this same document and server.</p>{/if}
      <p class="access-line"><strong>Effective access:</strong>
        {#if !caps.verified}Access not checked{:else if caps.can_edit}Can edit directly{:else if caps.can_comment}Can read and suggest changes{:else if caps.can_read}Read only{:else}Access unavailable{/if}
      </p>
    </div>
  </details>

  {#if connection.id}
    <div class="agent-actions">
      <div class="agent-action-buttons">
        <button class="btn btn-sm preset-tonal-surface" disabled={busy || connection.ended || !validDocumentLink()} onclick={() => void copyInstructions()}>{copied ? "Copied" : "Copy setup prompt"}</button>
        <details class="secondary-menu"><summary class="btn btn-sm preset-tonal-surface">More</summary><div class="menu-card"><button class="btn btn-sm" onclick={() => void newConversation()}>New conversation</button></div></details>
      </div>
    </div>
    {#if resetPending}<div class="reset-warning" role="alert">New conversation clears the visible transcript and draft, then creates fresh setup instructions. <button class="btn btn-sm preset-filled-primary-500" onclick={() => void newConversation()}>Clear and reset</button> <button class="btn btn-sm" onclick={() => resetPending = false}>Keep draft</button></div>{/if}
  {/if}

  {#if pendingRequest}
    <div class="request-warning" role="alert">
      <span>A new assistant request is ready. Replace the current draft and attached context?</span>
      <div class="setup-actions"><button class="btn btn-sm preset-filled-primary-500" onclick={() => applyRequest(pendingRequest)}>Replace draft and context</button><button class="btn btn-sm" onclick={() => pendingRequest = null}>Keep current draft</button></div>
    </div>
  {/if}

  <ChatTranscript messages={connection.messages} empty={connection.listening ? "No live messages yet." : "Connect your agent to start chatting."} roleLabel={(message) => message.role === "user" ? "You" : "Agent"} />

  <div class="agent-context">
    <div class="task-chips" role="group" aria-label="Assistant tasks">
      {#each [{kind:"proofread",label:"Proofread"},{kind:"tighten",label:"Tighten"},{kind:"rewrite",label:"Rewrite"},{kind:"explain",label:"Explain"}] as item}
        <button class="chip {task?.kind === item.kind ? 'selected' : ''}" disabled={!canPrepare(item.kind)} title={!canPrepare(item.kind) ? "Check access and attach the required context" : "Prepare this request"} onclick={() => chooseTask(item.kind)}>{item.label}</button>
      {/each}
    </div>
    <div class="scope-row" aria-label="Request scope">
      <span class="panel-meta">Scope:</span>
      {#each [{id:"selection",label:"Selected passage"},{id:"file",label:contextPath ? `File · ${contextPath}` : "Current file"},{id:"document",label:"Whole document"}] as item}
        <button class="scope-button" class:selected={scope === item.id} disabled={item.id === "selection" && !attachment || item.id === "file" && !contextPath} onclick={() => chooseScope(item.id)}>{item.label}</button>
      {/each}
    </div>
    {#if attachment}
      <div class="attachment">
        <div><strong>{attachment.anchored ? "Selected passage" : "Quoted passage"} · {attachment.path || path || "document"}</strong><blockquote>{selectedText}</blockquote></div>
        <div class="attachment-actions"><button class="btn btn-sm" onclick={removeSelection}>Remove</button><button class="btn btn-sm" disabled={!selection?.exact} onclick={replaceSelection}>Replace with current selection</button></div>
        {#if !attachment.anchored}<p class="panel-muted" role="alert">This passage has no source anchor. Explain remains available; Tighten and Rewrite need an anchored source selection.</p>{/if}
      </div>
    {/if}
    {#if diagnostic}<div class="diagnostic-context"><strong>Diagnostic · {contextPath}{diagnostic.line ? `:${diagnostic.line}` : ""}</strong><span>{diagnostic.message || "Explain this diagnostic"}</span><span>{diagnostic.source || ""}</span><button class="btn btn-sm" onclick={() => { diagnostic = null; diagnosticRevision = ""; }}>Remove diagnostic</button></div>{/if}
    <p class="scope-summary panel-meta">{scopeLabel(scope, { path: contextPath, attached: attachment })}</p>
  </div>

  <ChatComposer placeholder="Ask your agent…" canSend={!busy && sendable} draft={draft} ondraft={saveDraft} onsend={send} />
  {#if !sendable}<p class="panel-meta">{!connection.listening ? "You can draft now. Send becomes available when your agent is listening." : "This task needs the appropriate access and context. Choose another task or attach a passage."}</p>{/if}
  {#if reviewable.suggestions.length || reviewable.pass}<button class="btn preset-tonal-surface review-button" onclick={chooseResult}>Review suggestions{reviewable.suggestions.length ? ` (${reviewable.suggestions.length})` : ""}</button>{/if}
  <p class="conversation-notice panel-meta">Conversation isn't saved. The external agent or its provider may retain messages.</p>
  {#if problem || connection.error}<p class="panel-muted" role="alert">{problem || connection.error}</p>{/if}
</section>

<style>
  .agent-panel { display:flex; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); overflow-y:auto; overflow-x:hidden; }
  .agent-panel > :global(*) { flex-shrink:0; }
  .agent-status, .agent-setup, .setup-body, .agent-context { display:flex; flex-direction:column; gap:calc(var(--spacing) * 2); }
  .agent-status { padding:calc(var(--spacing) * 2); border-radius:var(--radius-container); background:var(--color-surface-100-900); }
  .agent-status span { overflow-wrap:anywhere; }
  .agent-setup { overflow:auto; max-height:30vh; }
  .agent-setup summary { cursor:pointer; font-weight:600; }
  .setup-body { padding-top:var(--spacing); }
  .setup-actions, .agent-actions, .agent-action-buttons, .attachment-actions { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .agent-actions { justify-content:flex-end; }
  .access-line { margin:0; }
  .task-chips, .scope-row { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .chip, .scope-button { border:1px solid var(--color-surface-300-700); border-radius:999px; background:transparent; padding:calc(var(--spacing) * .75) calc(var(--spacing) * 1.5); cursor:pointer; }
  .chip.selected, .scope-button.selected { border-color:var(--color-primary-500); background:color-mix(in srgb, var(--color-primary-500) 18%, transparent); }
  .chip:disabled, .scope-button:disabled { opacity:.5; cursor:not-allowed; }
  .attachment { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); border-left:3px solid var(--color-primary-500); background:var(--color-surface-100-900); }
  .attachment blockquote { max-height:7rem; overflow:auto; margin:var(--spacing) 0 0; white-space:pre-wrap; overflow-wrap:anywhere; }
  .diagnostic-context { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); background:var(--color-surface-100-900); }
  .diagnostic-context span { white-space:pre-wrap; overflow-wrap:anywhere; }
  .secondary-menu { position:relative; }
  .secondary-menu summary { list-style:none; }
  .secondary-menu summary::-webkit-details-marker { display:none; }
  .menu-card { position:absolute; z-index:2; right:0; top:calc(100% + var(--spacing)); padding:var(--spacing); background:var(--color-surface-100-900); box-shadow:var(--shadow-lg); }
  .reset-warning { padding:calc(var(--spacing) * 2); background:var(--color-warning-100-900); overflow-wrap:anywhere; }
  .request-warning { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); background:var(--color-warning-100-900); overflow-wrap:anywhere; }
  .conversation-notice { margin:0; }
  .review-button { align-self:flex-start; }
  .agent-panel :global(.chat-transcript-wrap) { flex:1 1 12rem; min-height:8rem; max-height:42vh; }
  @media (max-height:600px) { .agent-setup { max-height:20vh; } .attachment blockquote { max-height:4rem; } }
</style>
