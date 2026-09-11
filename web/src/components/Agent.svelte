<script>
  import { onMount } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { getPrivate, post } from "../lib/api.js";
  import PanelHeader from "./PanelHeader.svelte";
  import ChatTranscript from "./ChatTranscript.svelte";
  import ChatComposer from "./ChatComposer.svelte";
  import { createAgentClient } from "../lib/agent-client.js";
  import {
    captureAttachment, capabilityAllows, suggestionContext,
    normalizeCapabilities, scopeLabel, visibleResults,
  } from "../lib/assistant.js";

  let {
    slug, link, canShare = false, path = "", selection = null, revision = "", request = null,
    diagnostics = [], oncommenttask, ondiagnostictask,
    comments = [], suggestion: initialSuggestion = null, onreview, onpreview,
  } = $props();
  let client;
  let connection = $state({ id: "", token: "", messages: [], tasks: {}, connected: false,
    runnerConnected: false, status: "idle", error: "", capabilities: null });
  let busy = $state(false);
  let starting = $state(true);
  let problem = $state("");
  let attachment = $state(null);
  let suggestion = $state(null);
  $effect(() => { if (suggestion === null && initialSuggestion) suggestion = initialSuggestion; });
  let task = $state(null);
  let scope = $state("selection");
  let diagnostic = $state(null);
  let diagnosticRevision = $state("");
  let draft = $state("");
  let agentLink = $state("");
  let copyFallback = $state("");
  const roles = [
    { id: "reader", label: "Reader", help: "Read only" },
    { id: "commenter", label: "Commenter", help: "Read, comment, and suggest changes" },
    { id: "tracked", role: "commenter", label: "Edit with track changes", help: "Propose tracked changes for review; cannot edit source directly" },
    { id: "editor", label: "Edit directly", help: "Read, comment, and edit directly" },
  ];
  const currentRole = $derived(!caps.verified ? "" : caps.can_edit ? "editor" : caps.can_comment ? "commenter" : caps.can_read ? "reader" : "");
  let tab = $state("connection");
  let pendingRequest = $state(null);
  let commentContext = $state(null);
  let inputDraft = $state("");
  let inputDrafts = $state({});
  let lastRequestId = "";
  let lastSelection = $state(null);
  let suppressedSelection = $state("");
  let capabilityGeneration = 0;
  let verifiedCapabilities = $state(null);
  const previewResponses = new Map();
  const previewInFlight = new Set();
  const selectedText = $derived(attachment?.selection?.exact || "");
  const caps = $derived(normalizeCapabilities(verifiedCapabilities));
  const contextPath = $derived(diagnostic?.file || diagnostic?.path || path);
  const preparedTaskValid = $derived(!task || capabilityAllows(caps, task, {
    attached: attachment?.anchored ? attachment : (task.kind === "explain" ? attachment : null), path: contextPath,
  }));
  const uncertainDelivery = $derived(Object.values(connection.tasks || {}).some((item) =>
    item?.delivery === "uncertain" && item.request === draft.trim()));
  const sendable = $derived(connection.connected && connection.runnerConnected && preparedTaskValid && !uncertainDelivery);
  const chipScope = (kind) => ["tighten", "rewrite"].includes(kind) ? "selection"
    : kind === "proofread" ? (scope === "document" ? "document" : "file")
      : scope === "selection" && !attachment ? (contextPath ? "file" : "document") : scope;
  const canPrepare = (kind) => capabilityAllows(caps, { kind, scope: chipScope(kind) }, {
    attached: attachment?.anchored ? attachment : (kind === "explain" ? attachment : null), path: contextPath,
  });
  const interrupted = $derived(Object.values(connection.tasks || {}).some(task => task.status === "interrupted"));
  const status = $derived(
    !connection.connected || !connection.runnerConnected ? "Disconnected"
      : Object.values(connection.tasks || {}).some(task => task.status === "working") ? "Working"
        : interrupted ? "Interrupted" : "Waiting",
  );
  const inputTask = $derived(Object.values(connection.tasks || {}).find((item) => item?.status === "needs_input" && item.input) || null);

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

  function instructions(mode) {
    if (!connection.id || !connection.token || !validDocumentLink()) return "";
    const documentLink = agentLink || link;
    return [
      "Reuse recent LibrePaper CLI/version checks from this session when the installation has not changed; connecting another document does not require checking for a newer release. Otherwise check: librepaper --version && librepaper skills list && librepaper agent connect --help. If missing or these commands are unavailable, install or upgrade from https://github.com/LibrePaper/librepaper#install before continuing.",
      "Reuse LibrePaper skills already in context for the installed version. Load only missing skills with librepaper skills show librepaper-pair; librepaper skills show librepaper-document; librepaper skills show librepaper-write. Read references only as needed with librepaper skills show <name> --file references/<file>. Refresh affected instructions after an upgrade or a command/version mismatch. No separate skill installation is needed.",
      "Run each command below as written. Each is self-contained and can run in a separate shell.",
      `LIBREPAPER_CHAT_TOKEN=${shell(connection.token)} librepaper agent connect ${shell(documentLink)} ${shell(connection.id)} --background`,
      mode === "tracked"
        ? "Use track changes for every edit: create anchored LibrePaper suggestions for the user to accept or reject. Never write directly to document source or accept your own suggestions. This commenter link deliberately prevents direct source edits. Keep the runner attached and report suggestion identifiers in context.results."
        : mode === "editor"
          ? "Edit the document source directly when asked to make changes. Use the document and writing skills to verify edits. Keep this runner attached and report results in context.results."
          : "Keep this runner attached to the document. Use the document and writing skills for anchored suggestions and report result identifiers in context.results.",
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
    tab = "chat";
    saveDraft("");
    task = next.task || null;
    commentContext = next.comment || null;
    suggestion = next.suggestion || next.comment?.suggestion || null;
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
      commentContext = null;
      scope = next.diagnostic.file || next.diagnostic.path ? "file" : "document";
    }
    if (next.task) {
      task = { ...next.task };
      scope = next.task.scope || scope;
      if (!next.comment && !next.diagnostic) saveDraft(promptFor(task.kind, task.scope));
    }
    if (next.comment) {
      const quoted = next.comment.source || next.comment;
      if (quoted?.exact) attach({ ...quoted, revision: next.revision || next.comment.revision || revision }, next.revision || next.comment.revision || revision);
      else { suppressedSelection = selectionKey(selection || lastSelection); attachment = null; }
      scope = attachment ? "selection" : (contextPath ? "file" : "document");
      task = { kind: next.comment.suggestion ? "refine" : "respond", scope };
      saveDraft(next.comment.suggestion ? "Refine this suggestion." : "Address this comment.");
    }
    if (next.diagnostic) {
      task = next.task || { kind: "fix", scope };
      if (!draft.trim()) saveDraft(task.kind === "fix" ? "Fix this diagnostic." : "Explain this diagnostic.");
    }
    lastRequestId = next.id || lastRequestId;
  }

  function requestArrived(next) {
    if (!next?.id || next.id === lastRequestId) return;
    lastRequestId = next.id;
    tab = "chat";
    if (draft.trim()) pendingRequest = next;
    else applyRequest(next);
  }

  function chooseTask(kind) {
    const nextScope = chipScope(kind);
    if (!capabilityAllows(caps, { kind, scope: nextScope }, {
      attached: attachment?.anchored ? attachment : (kind === "explain" ? attachment : null), path: contextPath,
    })) return;
    tab = "chat";
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
    return `${{ proofread: "Proofread", tighten: "Tighten", rewrite: "Rewrite", explain: "Explain", fix: "Fix", refine: "Refine" }[kind] || kind} ${target}.`;
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
        suggestion: suggestionContext(suggestion),
        thread: ["respond", "refine"].includes(task?.kind) ? commentContext : null,
        path: task?.scope === "file" || !task ? contextPath : "",
        revision: diagnosticRevision || attachment?.revision || (task?.scope === "selection" ? revision : ""),
        diagnostic, diagnostics,
      });
      sent = true;
    });
    return sent;
  }

  async function copyInstructions(mode) {
    const role = mode === "tracked" ? "commenter" : mode;
    await act(async () => {
      copyFallback = "";
      if (canShare) {
        const endpoint = `/api/documents/${encodeURIComponent(slug)}/share`;
        let sharing = await getPrivate(endpoint);
        let access = sharing.links?.[role];
        if (!access?.key || access.expired) {
          sharing = await post(endpoint, { link: { role, until: "180d", label: "Agent" } });
          access = sharing.links?.[role];
        }
        if (!access?.url) throw new Error("Could not create an agent access link.");
        agentLink = new URL(access.url, location.origin).href;
      }
      if (!validDocumentLink()) throw new Error("The access link must point to this document.");
      await checkCapabilities(agentLink);
      if (currentRole !== role) throw new Error("This access level is unavailable. Ask the document owner for an access link.");
      try {
        await navigator.clipboard.writeText(instructions(mode));
      } catch { copyFallback = instructions(mode); }
    });
  }

  async function reconnect() { await act(() => client.reconnect()); }

  function uncertainTasks() { return Object.values(connection.tasks || {}).filter((item) => item?.delivery === "uncertain"); }
  async function retryTask(id) {
    await act(async () => {
      if (!await client.retry(id)) throw new Error("The original request is no longer available to retry.");
    });
  }
  async function answerInput(decision) {
    const item = inputTask;
    if (!item?.input?.request_id) return;
    const input = item.input;
    const response = input.kind === "approval"
      ? { decision }
      : { answers: Object.fromEntries((input.questions || []).map((question) => {
        const id = question.id || "answer";
        return [id, { answers: [String(inputDrafts[id] || inputDraft).trim()] }];
      })) };
    await act(() => client.respond(item.id, input.request_id, response));
    if (input.kind !== "approval") { inputDraft = ""; inputDrafts = {}; }
  }

  function chooseResult(results) {
    const reviewable = visibleResults({ context: { results } }, comments);
    if (reviewable.suggestions.length || reviewable.pass) onreview?.(reviewable);
    else problem = "These suggestions are no longer available.";
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
  $effect(() => {
    const preview = connection.previewRequest;
    if (!preview?.id || !onpreview || !client) return;
    const cached = previewResponses.get(preview.id);
    const sendResult = (result) => client.previewResult({
      ...result,
      id: `${preview.id}-result`, request_id: preview.id,
      task_id: preview.task_id,
      base_revision: preview.base_revision, revision: preview.revision,
    });
    // A runner retries an unconfirmed preview with the same request ID after
    // reconnecting. Reuse the immutable browser result and resend it rather
    // than compiling the candidate twice.
    if (cached) { void sendResult(cached); return; }
    if (previewInFlight.has(preview.id)) return;
    previewInFlight.add(preview.id);
    void (async () => {
      let result;
      try {
        // The relay carries only an immutable candidate handle. Source files
        // travel over authenticated same-origin requests, so a large file
        // never consumes the chat frame's bounded context budget.
        const candidate = preview.candidate_id
          ? await client.fetchCandidate(preview.candidate_id, undefined, preview.candidate_token || preview.renderer_token || "")
          : null;
        result = await onpreview(candidate ? { ...preview, candidate } : preview);
      } catch (error) {
        result = {
          ok: false,
          diagnostics: [{ severity: "error", message: error?.message || "Candidate verification failed." }],
          output: "none",
        };
      }
      previewResponses.set(preview.id, result);
      while (previewResponses.size > 8) previewResponses.delete(previewResponses.keys().next().value);
      previewInFlight.delete(preview.id);
      await sendResult(result);
    })();
  });
  async function checkCapabilities(nextLink = agentLink) {
    const generation = ++capabilityGeneration;
    verifiedCapabilities = null;
    // The browser keeps its own access link; the setup prompt grants the agent a separate role.
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
  <Tabs class="agent-tabs-root" value={tab} onValueChange={({ value }) => tab = value}
        ids={{ trigger: value => `agent-tab-${value}`, content: value => `agent-pane-${value}` }}>
    <Tabs.List class="agent-tabs" aria-label="Agent">
      {#each [{id:"connection",label:"Connection"},{id:"chat",label:"Chat"},{id:"tasks",label:"Tasks"}] as item}
        <Tabs.Trigger class="agent-tab" value={item.id}>{item.label}</Tabs.Trigger>
      {/each}
      <Tabs.Indicator class="agent-tab-indicator" />
    </Tabs.List>
  <Tabs.Content value="connection">
  <div class="agent-setup">
    <p class="panel-muted">Click a button below to copy connection instructions to your clipboard. Paste them into a new message in your local coding agent (Codex, Claude, Pi, etc.) and send it to connect the agent to this document. Choose <strong>Reader</strong> to let it read, <strong>Commenter</strong> to let it read, comment, and suggest changes, <strong>Edit with track changes</strong> to require suggestions you can accept or reject, or <strong>Edit directly</strong> to allow source edits.</p>
    <div class="access-buttons" role="group" aria-label="Copy setup prompt with access">
      {#each roles as role}
        <button class="btn btn-sm preset-tonal-surface" disabled={busy || starting || !connection.id || (!canShare && currentRole !== (role.role || role.id))} data-access={role.id} title={role.help} onclick={() => void copyInstructions(role.id)}>{role.label}</button>
      {/each}
    </div>
    {#if copyFallback}<textarea class="input setup-prompt" readonly rows="6" aria-label="Setup prompt to copy" value={copyFallback} onclick={(event) => event.currentTarget.select()}></textarea>{/if}
  </div>
  {#if !connection.id}<button class="btn preset-filled-primary-500" disabled={busy || starting} onclick={() => void act(() => client.create())}>{starting ? "Connecting…" : "Retry connection"}</button>{/if}
  {#if connection.id && !connection.connected}
    <div role="status"><span class="panel-muted">Reconnecting…</span> <button class="btn btn-sm preset-tonal-surface" disabled={busy} onclick={() => void reconnect()}>Reconnect now</button></div>
  {/if}

  </Tabs.Content>

  <Tabs.Content value="chat" class="agent-tab-content">
    <div class="chat-history">
    <div class="agent-actions">
      <span class="panel-meta" role="status">{status}{#if status === "Working"}<span class="working-dots" aria-hidden="true"><span>.</span><span>.</span><span>.</span></span>{/if}</span>
    </div>

  {#if pendingRequest}
    <div class="request-warning" role="alert">
      <span>A new assistant request is ready. Replace the current draft and attached context?</span>
      <div class="setup-actions"><button class="btn btn-sm preset-filled-primary-500" onclick={() => applyRequest(pendingRequest)}>Replace draft and context</button><button class="btn btn-sm" onclick={() => pendingRequest = null}>Keep current draft</button></div>
    </div>
  {/if}

  {#each uncertainTasks() as item (item.id)}
    <p class="panel-meta" role="alert">Message delivery was not confirmed. <button class="btn btn-sm preset-tonal-surface" disabled={busy} onclick={() => void retryTask(item.id)}>Retry delivery</button></p>
  {/each}
  <ChatTranscript messages={connection.messages} empty={connection.runnerConnected ? "No messages yet." : "Connect your agent in the Connection tab to begin."} roleLabel={(message) => message.role === "user" ? "You" : "Agent"} onresult={chooseResult} />

  {#if inputTask}
    <div class="input-request" role="group" aria-label="Assistant input request">
      <strong>{inputTask.input.kind === "approval" ? "The assistant requests approval" : "The assistant needs an answer"}</strong>
      {#if inputTask.input.message}<p>{inputTask.input.message}</p>{/if}
      {#if inputTask.input.kind === "approval"}
        <div class="setup-actions"><button class="btn btn-sm preset-filled-primary-500" disabled={busy} onclick={() => void answerInput("accept")}>Approve</button><button class="btn btn-sm" disabled={busy} onclick={() => void answerInput("decline")}>Deny</button></div>
      {:else}
        {#each inputTask.input.questions || [] as question, index (question.id || `question-${index}`)}
          {@const questionId = question.id || "answer"}
          <label class="label">{question.question}<textarea class="input" value={inputDrafts[questionId] || ""} oninput={(event) => inputDrafts[questionId] = event.currentTarget.value} rows="3" placeholder={question.header || "Answer"}></textarea></label>
        {/each}
        <button class="btn btn-sm preset-filled-primary-500" disabled={busy || !(inputTask.input.questions || []).length || !(inputTask.input.questions || []).every((question, index) => (inputDrafts[question.id || "answer"] || inputDraft).trim())} onclick={() => void answerInput()}>Send answer</button>
      {/if}
    </div>
  {/if}

  {#if task || attachment}<p class="panel-meta">{scopeLabel(scope, { path: contextPath, attached: attachment })} <button class="btn btn-sm" onclick={() => tab = "tasks"}>Change context</button></p>{/if}
  {#if !sendable}<p class="panel-meta">{uncertainDelivery ? "This request was not confirmed. Retry delivery above before sending it again." : !connection.runnerConnected ? "You can draft now. Connect your agent to send." : "This task needs the appropriate access and context. Choose another task or attach a passage."}</p>{/if}
    </div>
  <ChatComposer placeholder="Ask your agent…" canSend={!busy && sendable} draft={draft} ondraft={saveDraft} onsend={send} />
  </Tabs.Content>
  <Tabs.Content value="tasks" class="agent-tab-content">
    <p class="panel-muted">Choose a task to prepare a message for your agent.</p>
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
    {#if oncommenttask && comments.some(comment => !comment.resolved)}
      <div class="context-tasks">
        <h3 class="panel-section-title">Comments</h3>
        {#each comments.filter(comment => !comment.resolved) as comment (comment.id)}
          <div>
            <p class="panel-muted">{comment.body || comment.exact || "Suggestion"}</p>
            <button class="btn btn-sm preset-tonal-surface" disabled={!caps.can_comment} onclick={() => oncommenttask(comment)}>{comment.motivation === "editing" ? "Refine suggestion" : "Address comment"}</button>
          </div>
        {/each}
      </div>
    {/if}
    {#if ondiagnostictask && diagnostics.length}
      <div class="context-tasks">
        <h3 class="panel-section-title">Diagnostics</h3>
        {#each diagnostics as item}
          <div>
            <p class="panel-muted">{item.message}</p>
            <button class="btn btn-sm preset-tonal-surface" disabled={!caps.can_comment} onclick={() => ondiagnostictask(item)}>Fix diagnostic</button>
          </div>
        {/each}
      </div>
    {/if}
    <p class="scope-summary panel-meta">{scopeLabel(scope, { path: contextPath, attached: attachment })}</p>
  </div>

  </Tabs.Content>
  </Tabs>
  {#if problem || connection.error}<p class="panel-muted" role="alert">{problem || connection.error}</p>{/if}
</section>

<style>
  .agent-panel { display:flex; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); overflow:hidden; }
  .agent-panel > :global(*) { flex-shrink:0; }
  .agent-panel :global(.agent-tabs-root) { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); }
  .agent-panel :global(.agent-tabs) { position:relative; display:flex; flex-shrink:0; border-bottom:1px solid var(--color-surface-300-700); }
  .agent-panel :global(.agent-tab) { flex:1; min-width:0; padding:calc(var(--spacing) * 2) var(--spacing); cursor:pointer; color:var(--color-surface-500-500); }
  .agent-panel :global(.agent-tab[data-selected]) { color:var(--color-primary-700-300); font-weight:700; background:color-mix(in srgb, var(--color-primary-500) 18%, transparent); box-shadow:inset 0 -3px 0 var(--color-primary-500); }
  .agent-panel :global(.agent-tab-indicator) { height:3px; bottom:0; background:var(--color-primary-500); }
  .agent-panel :global([role="tabpanel"]) { flex:1 1 0; min-height:0; overflow-y:auto; }
  .agent-panel :global(#agent-pane-chat) { overflow:hidden; }
  .chat-history { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); overflow-y:auto; }
  .chat-history > :global(*) { flex-shrink:0; }
  .agent-panel :global(.agent-tab-content .chat-form) { flex-shrink:0; }
  .agent-panel :global(.agent-tab-content) { display:flex; flex-direction:column; gap:calc(var(--spacing) * 3); min-height:0; }
  .agent-panel :global([hidden]) { display:none; }
  .agent-setup, .agent-context { display:flex; flex-direction:column; gap:calc(var(--spacing) * 2); }
  .agent-setup p { margin:0; }
  .access-buttons { display:grid; grid-template-columns:repeat(2, minmax(0, 1fr)); gap:var(--spacing); }
  .access-buttons button { min-width:0; padding-inline:var(--spacing); white-space:normal; }
  .setup-prompt { width:100%; min-width:0; }
  .setup-actions, .agent-actions, .attachment-actions { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .agent-actions { justify-content:space-between; }
  .working-dots span { animation:working-dot 1.2s infinite; }
  .working-dots span:nth-child(2) { animation-delay:.2s; }
  .working-dots span:nth-child(3) { animation-delay:.4s; }
  @keyframes working-dot { 0%, 80%, 100% { opacity:.25; } 40% { opacity:1; } }
  @media (prefers-reduced-motion:reduce) { .working-dots span { animation:none; } }
  .context-tasks { display:flex; flex-direction:column; gap:calc(var(--spacing) * 3); }
  .context-tasks p { overflow-wrap:anywhere; margin:0 0 var(--spacing); }
  .task-chips, .scope-row { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .chip, .scope-button { border:1px solid var(--color-surface-300-700); border-radius:999px; background:transparent; padding:calc(var(--spacing) * .75) calc(var(--spacing) * 1.5); cursor:pointer; }
  .chip.selected, .scope-button.selected { border-color:var(--color-primary-500); background:color-mix(in srgb, var(--color-primary-500) 18%, transparent); }
  .chip:disabled, .scope-button:disabled { opacity:.5; cursor:not-allowed; }
  .attachment { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); border-left:3px solid var(--color-primary-500); background:var(--color-surface-100-900); }
  .attachment blockquote { max-height:7rem; overflow:auto; margin:var(--spacing) 0 0; white-space:pre-wrap; overflow-wrap:anywhere; }
  .diagnostic-context { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); background:var(--color-surface-100-900); }
  .diagnostic-context span { white-space:pre-wrap; overflow-wrap:anywhere; }
  .request-warning { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); background:var(--color-warning-100-900); overflow-wrap:anywhere; }
  .input-request { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); border-left:3px solid var(--color-warning-500); background:var(--color-surface-100-900); }
  .input-request p { margin:0; white-space:pre-wrap; overflow-wrap:anywhere; }
  .agent-panel :global(.chat-transcript-wrap) { flex:1 0 8rem; min-height:8rem; }
  @media (max-height:600px) { .attachment blockquote { max-height:4rem; } }
</style>
