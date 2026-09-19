<script>
  import { onMount } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { getPrivate, post } from "../../lib/api.js";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import ChatTranscript from "../ChatTranscript.svelte";
  import ChatComposer from "../ChatComposer.svelte";
  import { createAgentClient } from "../../lib/agent-client.js";
  import * as local from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";
  import {
    captureAttachment, capabilityAllows, diagnosticLabel, suggestionContext,
    normalizeCapabilities, scopeLabel, visibleResults,
    findTask, groupTasks, inferScope, searchTasks, taskPrompt, taskScopes,
  } from "../../lib/assistant.js";

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
  // The Tasks pane is a launcher: a searchable catalog, then a preparation
  // view for the one task the user picked. Nothing is written into the draft
  // until that view is confirmed.
  let taskView = $state("launcher");
  let taskQuery = $state("");
  let chosenTaskId = $state("");
  let extra = $state("");
  let agentLink = $state("");
  // What this computer has, as the local app reports it. The browser cannot
  // read a PATH, so until the app is paired there is nothing to offer and the
  // panel says so rather than guessing.
  let installed = $state([]);
  let connectionName = $state("");
  let chosenAgent = $state("");
  let assistant = $state({ running: false, agent: "", access: "" });
  let pairingCode = $state("");
  let pairProblem = $state("");
  let appAddressDraft = $state(local.address());
  // The local app's state drives the whole connection panel, so this view
  // holds its own subscription and asks once itself. Subscribing alone only
  // schedules the periodic reconnect, so a panel that waited for someone
  // else's probe would sit on "install the app" for up to fifteen seconds
  // with the app already running.
  $effect(() => companion.watch());
  // Which document the local app is asked about is page-level state the
  // Reader sets when it loads one; this panel does not reach for it. The
  // probe does belong here: the panel is only mounted once the reader opens
  // it, so asking then is a response to their gesture rather than something
  // opening a document does on its own.
  onMount(() => { void local.probe().catch(() => {}); });
  const paired = $derived(companion.status?.state === "connected");
  const reachable = $derived(["reachable", "unauthorized", "connected", "incompatible"].includes(companion.status?.state));
  const chosenInstalled = $derived(installed.find((entry) => entry.id === chosenAgent) || null);
  // The four access levels, as a choice rather than four commands: each row
  // carries its own explanation, so the panel never defines them twice.
  // No read-only level. The document's MCP surface admits a commenter at
  // minimum and answers a reader link with a flat 404 (server/mcp.rs), so an
  // assistant on a read link starts, receives no tools at all and cannot say
  // why. Offering a level the product cannot serve is worse than not offering
  // it: Comment is the narrowest that works, and it still writes nothing the
  // reader has not accepted.
  const roles = [
    { id: "commenter", label: "Comment", help: "Read, comment, and suggest changes",
      detail: "The agent can leave comments and attach suggestions for you to accept or reject." },
    { id: "tracked", role: "commenter", label: "Track changes", help: "Changes require your approval",
      detail: "The agent proposes source changes as tracked suggestions. You review and accept or reject each one." },
    { id: "editor", label: "Edit", help: "Changes do not require approval", warn: true,
      detail: "The agent writes to the document source directly, with no accept or reject step in between." },
  ];
  const currentRole = $derived(!caps.verified ? "" : caps.can_edit ? "editor" : caps.can_comment ? "commenter" : caps.can_read ? "reader" : "");
  // Without sharing rights the panel can only hand out the access the reader
  // already holds, so the other rows are visible but not selectable.
  const roleOffered = (entry) => canShare || currentRole === (entry.role || entry.id);
  let access = $state("");
  const chosenAccess = $derived(roles.find((entry) => entry.id === access) || null);
  // Running "here" means running as what is currently selected. A different
  // agent, or a different access level, is a different assistant: the one
  // running still holds the link it was started with, so the panel must offer
  // to restart rather than claim the selection is already in force.
  const runningHere = $derived(assistant.running && assistant.agent === chosenAgent && assistant.access === access);
  const runningLabel = $derived(installed.find((entry) => entry.id === assistant.agent)?.label || "assistant");
  let tab = $state("connection");
  const TABS = [
    { id: "connection", label: "Connection" },
    { id: "chat", label: "Chat" },
    { id: "tasks", label: "Tasks" },
  ];
  let pendingRequest = $state(null);
  let commentContext = $state(null);
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
  const runnerReady = $derived(connection.connected && connection.runnerConnected);
  const sendable = $derived(runnerReady && preparedTaskValid && !uncertainDelivery);
  const taskGroups = $derived(groupTasks(searchTasks(taskQuery)));
  const chosen = $derived(findTask(chosenTaskId));
  const scopeAvailable = (id) => id === "selection" ? Boolean(attachment) : id === "file" ? Boolean(contextPath) : true;
  const allows = (kind, chosenScope) => capabilityAllows(caps, { kind, scope: chosenScope }, {
    attached: attachment?.anchored ? attachment : (kind === "explain" ? attachment : null), path: contextPath,
  });
  // A row is offered when some scope the reader can actually supply works for
  // it; which scope that is belongs to the preparation view, not the catalog.
  const canPrepare = (entry) => taskScopes(entry).some((id) => scopeAvailable(id) && allows(entry.kind, id));
  const preparable = $derived(Boolean(chosen) && scopeAvailable(scope) && allows(chosen.kind, scope));
  const interrupted = $derived(Object.values(connection.tasks || {}).some(task => task.status === "interrupted"));
  const status = $derived(
    !connection.connected || !connection.runnerConnected ? "Disconnected"
      : Object.values(connection.tasks || {}).some(task => task.status === "working") ? "Working"
        : interrupted ? "Interrupted" : "Waiting",
  );
  const inputTask = $derived(Object.values(connection.tasks || {}).find((item) => item?.status === "needs_input" && item.input) || null);

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

  /// Mint the access link for `mode`, confirm it really grants that access,
  /// and hand it to the local app once. Everything after this refers to the
  /// document by the name the app gives back, so no key reaches a config
  /// file, a command line or a chat transcript.
  async function registerConnection(mode) {
    const role = mode === "tracked" ? "commenter" : mode;
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
    const answer = await local.registerConnection({
      title: document.title || slug, link: agentLink || link, access: mode,
    });
    if (!answer?.connection) throw new Error(answer?.error || "The local app could not register this document.");
    connectionName = answer.connection;
    return connectionName;
  }

  async function refreshAgents() {
    if (!paired) { installed = []; return; }
    const answer = await local.agents();
    installed = Array.isArray(answer?.agents) ? answer.agents : [];
    if (!chosenAgent) chosenAgent = installed.find((entry) => entry.assistant)?.id || installed[0]?.id || "";
  }
  $effect(() => { if (paired) void act(refreshAgents); });

  async function pair() {
    pairProblem = "";
    busy = true;
    try {
      await local.connect(pairingCode.trim());
      pairingCode = "";
      await refreshAgents();
    } catch (error) {
      // Said here rather than through `act`, whose message lands at the foot
      // of the panel where the person pairing will not see it.
      const detail = error?.message || String(error);
      pairProblem = /unauthor|403|invalid|code/i.test(detail)
        ? `${detail} The app prints its code when it starts; a second copy of the app has a different one.`
        : detail;
    } finally {
      busy = false;
    }
  }

  /// Point this browser at a local app that is not on the default port, and
  /// look again immediately so the panel answers rather than waiting for the
  /// next scheduled probe.
  async function useAppAddress() {
    pairProblem = "";
    await act(async () => {
      local.setAddress(appAddressDraft.trim());
      appAddressDraft = local.address();
      const status = await local.probe({ force: true });
      if (status?.state === "unreachable" || status?.state === "denied") {
        throw new Error(`No local app answers at ${local.address()}`);
      }
      if (status?.state === "connected") await refreshAgents();
    });
  }

  /// Start the assistant: mint the access link, hand it to the local app as a
  /// named connection, and have the app drive the chosen agent against it.
  /// This is the only way an agent is connected, so it is the only button.
  async function startAssistant(mode) {
    await act(async () => {
      // One assistant per conversation. Starting a second against the same
      // one would leave the first holding the link it was started with, so a
      // change of agent or access replaces it rather than racing it.
      if (assistant.running && connectionName) {
        await local.stopAssistant({ connection: connectionName, conversation: connection.id })
          .catch(() => {});
        assistant = { running: false, agent: "", access: "" };
      }
      const name = connectionName || await registerConnection(mode);
      const answer = await local.startAssistant({
        connection: name, conversation: connection.id,
        chatToken: connection.token, agent: chosenAgent,
      });
      if (!answer?.running) throw new Error(answer?.error || "The assistant did not start.");
      assistant = { running: true, agent: chosenAgent, access: mode };
      // The work happens in the chat, so go there rather than leaving the
      // reader on a settings pane that has nothing more to say.
      tab = "chat";
    });
  }

  async function stopAssistant() {
    await act(async () => {
      await local.stopAssistant({ connection: connectionName, conversation: connection.id });
      assistant = { running: false, agent: "", access: "" };
    });
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
    // A contextual action prepares the same shape as a catalog task, but it
    // is not one of them: leave the launcher on its list.
    chosenTaskId = "";
    taskView = "launcher";
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

  function openTask(entry) {
    if (!entry || !canPrepare(entry)) return;
    chosenTaskId = entry.id;
    extra = "";
    scope = inferScope(entry, { attached: attachment, path: contextPath });
    taskView = "prepare";
  }

  function closeTask() {
    taskView = "launcher";
  }

  /// Picking a task is the decision; writing a draft the reader then has to
  /// send again was a second confirmation of the same thing. This sends it and
  /// moves to the chat, where the answer will arrive.
  async function sendTask() {
    // Deliberately not gated on `sendable`: that judges the task currently in
    // context, which is the previous one. What matters is whether the task
    // being sent is allowed, which is `preparable`, and whether the assistant
    // is there. `send` makes the final check once this task is in context.
    if (!chosen || !preparable || !runnerReady) return;
    const entry = chosen;
    task = { kind: entry.kind, scope };
    const note = extra.trim();
    const prompt = taskPrompt(entry, scope);
    if (entry.kind !== "explain") { diagnostic = null; diagnosticRevision = ""; }
    // Leave the catalog showing for the next request; "Change context" in the
    // chat pane reopens this task's view.
    taskView = "launcher";
    tab = "chat";
    saveDraft("");
    await send(note ? `${prompt}\n\n${note}` : prompt);
  }

  function promptFor(kind, chosenScope) {
    const target = chosenScope === "document" ? "the whole document" : chosenScope === "file" ? "this file" : "the selected passage";
    return `${{ proofread: "Proofread", tighten: "Tighten", rewrite: "Rewrite", explain: "Explain", fix: "Fix", refine: "Refine" }[kind] || kind} ${target}.`;
  }

  // Scope is chosen inside the preparation view, before anything is written
  // into the draft, so changing it neither rewrites the composer nor discards
  // the attached passage the user may switch back to.
  function chooseScope(next) {
    scope = next;
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

  function chooseAgent(id) {
    chosenAgent = id;
  }

  function chooseAccess(id) {
    access = id;
    // A connection carries one access level. Reusing the previous one here
    // would silently hand out the old link after the user picked a narrower
    // level, which is the one mistake this panel must never make.
    connectionName = "";
  }

  async function reconnect() { await act(() => client.reconnect()); }

  function uncertainTasks() { return Object.values(connection.tasks || {}).filter((item) => item?.delivery === "uncertain"); }
  async function retryTask(id) {
    await act(async () => {
      if (!await client.retry(id)) throw new Error("The original request is no longer available to retry.");
    });
  }
  /// Answer a permission request with one of the options the agent offered,
  /// or `null` to cancel it. The runner rejects anything else, so the two
  /// sides cannot drift into answering a different question.
  async function answerInput(option) {
    const item = inputTask;
    if (!item?.input?.request_id) return;
    const response = option ? { option } : { cancelled: true };
    await act(() => client.respond(item.id, item.input.request_id, response));
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

<section class="panel panel-tabbed agent-panel" aria-label="Agent chat">
  <PanelHeader title="Agent" />
  <!-- The strip, its ids and the layout of a pane are PanelTabs', shared with
       the collaboration panel. What is left here is what the tabs contain. -->
  <PanelTabs id="agent" label="Agent" listClass="agent-tabs" tabs={TABS}
             value={tab} onchange={(value) => tab = value}>
  <Tabs.Content value="connection">
  <div class="agent-setup">
    <div class="setup-intro">
      <h3 class="setup-title">Connect a coding agent</h3>
      <p class="panel-muted">Work on this document with a coding agent already installed on your computer. It uses its own model and its own sign-in.</p>
    </div>

    <!-- Where the app is listening. The default port can be taken, and the
         app is then started on another one with `local start --port`; without
         this the panel would insist the app is missing while it is running a
         port away, and the setting that fixes it lives in another screen. -->
    {#snippet appAddress()}
      <details class="setup-address">
        <summary class="panel-meta">Started the app on another port?</summary>
        <label class="label"><span class="sr-only">Local app address</span>
          <input class="input" type="text" aria-label="Local app address" value={appAddressDraft}
                 oninput={(event) => appAddressDraft = event.currentTarget.value} /></label>
        <button class="btn btn-sm preset-outlined-surface-300-700" disabled={busy}
                onclick={() => void useAppAddress()}>Look there instead</button>
      </details>
    {/snippet}

    <!-- Everything below hangs off the local app being paired. That is the
         one manual step, it is done once per computer rather than once per
         document, and it is the same pairing the local compiler uses, so a
         reader who already paired for Quarto or native TeX skips it. -->
    {#if !paired}
      <div class="setup-requirement">
        {#if reachable}
          <strong>Connect the LibrePaper app on this computer</strong>
          <span class="panel-meta">The app shows a six digit code when it starts. Enter it once; this browser stays paired afterwards.</span>
          <div class="setup-actions">
            <label class="label"><span class="sr-only">Pairing code</span>
              <input class="input setup-code" inputmode="numeric" autocomplete="one-time-code" maxlength="8"
                     placeholder="000000" value={pairingCode}
                     oninput={(event) => pairingCode = event.currentTarget.value} /></label>
            <button class="btn btn-sm preset-filled-primary-500" disabled={busy || pairingCode.trim().length < 6}
                    onclick={() => void pair()}>Pair</button>
          </div>
          <!-- A refused code must say so here, beside the button that was
               pressed. The panel's shared error line sits below three tabs of
               content, which in a narrow column is off screen: an invisible
               error reads as a dead button. -->
          {#if pairProblem}<span class="panel-meta" role="alert">{pairProblem}</span>{/if}
          {@render appAddress()}
        {:else}
          <strong>Install the LibrePaper app on this computer</strong>
          <code class="setup-command">curl -fsSL https://librepaper.org/install.sh | sh</code>
          <span class="panel-meta">Run that once, in a terminal. It prints a pairing code when it finishes, and this panel then lists the coding agents you already have installed.</span>
          <a href="https://github.com/LibrePaper/librepaper#install" target="_blank" rel="noreferrer">Other ways to install →</a>
          {@render appAddress()}
        {/if}
      </div>
    {:else}
      <div class="setup-requirement" data-state="ready">
        <strong>Connected to this computer</strong>
        {#if installed.length}
          <!-- Two settings, each a list of mutually exclusive values with one
               in force: a select says which is chosen by showing it, where a
               row of buttons has to signal it and can fail to. -->
          <label class="label setup-field">
            <span class="setup-label">Agent</span>
            <select class="select" aria-label="Agent" disabled={busy} value={chosenAgent}
                    onchange={(event) => chooseAgent(event.currentTarget.value)}>
              {#each installed as entry (entry.id)}
                <option value={entry.id}>{entry.label}</option>
              {/each}
            </select>
          </label>
          {#if chosenInstalled}
            <span class="panel-meta" data-agent-note={chosenInstalled.id}>
              {chosenInstalled.assistant ? "Brings its own model and sign-in; can also run in the sidebar." : "Brings its own model and sign-in. Document tools only."}
              {chosenInstalled.assistant_blocked}{chosenInstalled.assistant_note}
            </span>
          {/if}
        {:else}
          <span class="panel-meta">No supported coding agent was found on this computer. Install one, or connect your own with the configuration snippet below.</span>
        {/if}
      </div>
    {/if}

    <label class="label setup-field">
      <span class="setup-label">Access</span>
      <select class="select" aria-label="Access" value={access}
              disabled={busy || starting || !connection.id}
              onchange={(event) => chooseAccess(event.currentTarget.value)}>
        <option value="" disabled>Choose what the agent may do</option>
        {#each roles as entry}
          <option value={entry.id} disabled={!roleOffered(entry)}>{entry.label}: {entry.help}</option>
        {/each}
      </select>
    </label>

    {#if chosenAccess}
      <div class="access-detail" data-access={chosenAccess.id}>
        <p class="panel-muted">{chosenAccess.detail}</p>
        <!-- One destination. An agent is used here, in the document, where
             the chat and the task launcher are; there is no second place to
             send it to and so no second button to explain. -->
        <div class="setup-actions">
          <button class="btn btn-sm preset-filled-primary-500"
                  disabled={busy || starting || !connection.id || !paired || !chosenInstalled?.assistant || runningHere}
                  onclick={() => void startAssistant(chosenAccess.id)}>
            {runningHere ? "Running"
              : assistant.running ? "Restart with this choice"
              : "Start assistant"}
          </button>
          {#if assistant.running}
            <!-- Named, because the assistant may be running on an agent other
                 than the one currently selected, and "Stop" with no subject
                 would be a guess about which. -->
            <button class="btn btn-sm" disabled={busy} onclick={() => void stopAssistant()}>
              Stop {runningLabel}
            </button>
          {/if}
        </div>
        <!-- What pressing it does, and what it costs. The fetch warning lives
             here rather than in the label so it survives the label changing
             to "Restart with this choice", which is exactly when a reader is
             switching to an agent whose adapter is not yet on the machine. -->
        <span class="panel-meta">
          {runningHere
            ? "Ask for changes in the Chat tab, or pick one from Tasks."
            : `LibrePaper runs ${chosenInstalled?.label || "the agent"} against this document. You work with it in the Chat and Tasks tabs.`}
          {#if !runningHere && chosenInstalled?.assistant_fetches}Starting it downloads its adapter the first time.{/if}
          <!-- A runner outlives the page that started it, so after a reload
               one can be attached that this panel never started. Saying so
               beats offering Start as though nothing were running. -->
          {#if !assistant.running && connection.runnerConnected}An assistant is already attached to this conversation; starting one replaces it.{/if}
        </span>
      </div>
    {/if}
  </div>
  {#if !connection.id}<button class="btn preset-filled-primary-500" disabled={busy || starting} onclick={() => void act(() => client.create())}>{starting ? "Connecting…" : "Retry connection"}</button>{/if}
  {#if connection.id && !connection.connected}
    <div role="status"><span class="panel-muted">Reconnecting…</span> <button class="btn btn-sm preset-outlined-surface-300-700" disabled={busy} onclick={() => void reconnect()}>Reconnect now</button></div>
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
    <p class="panel-meta" role="alert">Message delivery was not confirmed. <button class="btn btn-sm preset-outlined-surface-300-700" disabled={busy} onclick={() => void retryTask(item.id)}>Retry delivery</button></p>
  {/each}
  <ChatTranscript messages={connection.messages} empty={connection.runnerConnected ? "No messages yet." : "Connect your agent in the Connection tab to begin."} roleLabel={(message) => message.role === "user" ? "You" : "Agent"} onresult={chooseResult} />

  {#if inputTask}
    <div class="input-request" role="group" aria-label="Assistant input request">
      <strong>The assistant requests permission</strong>
      {#if inputTask.input.message}<p>{inputTask.input.message}</p>{/if}
      <!-- The agent names its own options, so the sidebar renders exactly
           those. Inventing an "Approve" it never offered would be answering
           a question the agent did not ask. -->
      <div class="setup-actions">
        {#each inputTask.input.options || [] as option, index (option.id || `option-${index}`)}
          <button class="btn btn-sm {index === 0 ? "preset-filled-primary-500" : ""}" disabled={busy}
                  onclick={() => void answerInput(option.id)}>{option.label}</button>
        {/each}
        <button class="btn btn-sm" disabled={busy} onclick={() => void answerInput(null)}>Cancel</button>
      </div>
    </div>
  {/if}

  <!-- The draft carries its own context, so the chat pane states it in one
       line and sends the user back to the task view to change it. -->
  {#if task || attachment}<p class="panel-meta context-summary">{scopeLabel(scope, { path: contextPath, attached: attachment })} <button class="btn btn-sm" onclick={() => { tab = "tasks"; taskView = chosen ? "prepare" : "launcher"; }}>Change context</button>{#if attachment}<button class="btn btn-sm" onclick={removeSelection}>Remove passage</button>{/if}</p>{/if}
  {#if !sendable}<p class="panel-meta">{uncertainDelivery ? "This request was not confirmed. Retry delivery above before sending it again." : !connection.runnerConnected ? "You can draft now. Connect your agent to send." : "This task needs the appropriate access and context. Choose another task or attach a passage."}</p>{/if}
    </div>
  <ChatComposer placeholder="Ask your agent…" canSend={!busy && sendable} draft={draft} ondraft={saveDraft} onsend={send} />
  </Tabs.Content>
  <Tabs.Content value="tasks" class="agent-tab-content">
  {#if taskView === "prepare" && chosen}
    <div class="agent-context task-prepare">
      <button class="task-back" onclick={closeTask}>← All tasks</button>
      <div>
        <h3 class="task-title">{chosen.label}</h3>
        <p class="panel-muted task-description">{chosen.description}</p>
      </div>
      <label class="label task-scope">Scope
        <select class="select" value={scope} onchange={(event) => chooseScope(event.currentTarget.value)}>
          {#each taskScopes(chosen) as id}
            <option value={id} disabled={!scopeAvailable(id)}>{scopeLabel(id, { path: contextPath, attached: id === "selection" ? attachment : null })}</option>
          {/each}
        </select>
      </label>
      {#if scope === "selection" && attachment}
        <div class="attachment">
          <div><strong>{attachment.anchored ? "Selected passage" : "Quoted passage"} · {attachment.path || path || "document"}</strong><blockquote>{selectedText}</blockquote></div>
          <div class="attachment-actions"><button class="btn btn-sm" onclick={removeSelection}>Remove</button><button class="btn btn-sm" disabled={!selection?.exact} onclick={replaceSelection}>Replace with current selection</button></div>
          {#if !attachment.anchored}<p class="panel-muted" role="alert">This passage has no source anchor. Explain remains available; Tighten and Rewrite need an anchored source selection.</p>{/if}
        </div>
      {/if}
      {#if diagnostic}<div class="diagnostic-context"><strong>Diagnostic · {contextPath}{diagnostic.line ? `:${diagnostic.line}` : ""}</strong><span>{diagnostic.message || "Explain this diagnostic"}</span><span>{diagnostic.source || ""}</span><button class="btn btn-sm" onclick={() => { diagnostic = null; diagnosticRevision = ""; }}>Remove diagnostic</button></div>{/if}
      <label class="label">Additional instructions
        <textarea class="input" rows="3" value={extra} oninput={(event) => extra = event.currentTarget.value} placeholder="Optional"></textarea>
      </label>
      {#if !preparable}<p class="panel-meta" role="status">This task needs the appropriate access and context. Choose another scope, or attach a passage in the document.</p>
      {:else if !runnerReady}<p class="panel-meta" role="status">Start the assistant in the Connection tab before sending a task.</p>{/if}
      <div class="prepare-actions"><button class="btn btn-sm preset-filled-primary-500" disabled={busy || !preparable || !runnerReady} onclick={() => void sendTask()}>Send to agent</button></div>
    </div>
  {:else}
    <div class="agent-context task-launcher">
      <p class="panel-muted">Ask your agent to…</p>
      <input class="input task-search" type="search" placeholder="Search tasks…" aria-label="Search tasks"
             value={taskQuery} oninput={(event) => taskQuery = event.currentTarget.value} />
      <div class="task-list task-catalog" role="group" aria-label="Assistant tasks">
        {#each taskGroups as group (group.category)}
          <h3 class="task-group">{group.category}</h3>
          {#each group.tasks as entry (entry.id)}
            <button class="task-row" disabled={!canPrepare(entry)}
                    title={canPrepare(entry) ? entry.description : "Check access and attach the required context"}
                    onclick={() => openTask(entry)}>
              <span>{entry.label}</span><span class="task-chevron" aria-hidden="true">›</span>
            </button>
          {/each}
        {/each}
        {#if !taskGroups.length}<p class="panel-muted">No task matches “{taskQuery}”.</p>{/if}
      </div>

      {#if (oncommenttask && comments.some(comment => !comment.resolved)) || (ondiagnostictask && diagnostics.length)}
        <!-- Not peers of the catalog: these are openings the document itself
             offers, which prepare the same task + context + instructions. -->
        <div class="context-tasks task-list">
          <h3 class="task-group">Contextual</h3>
          {#if oncommenttask}
            {#each comments.filter(comment => !comment.resolved) as comment (comment.id)}
              <div class="context-task">
                <p class="panel-muted">{comment.body || comment.exact || "Suggestion"}</p>
                <button class="task-row" disabled={!caps.can_comment} onclick={() => oncommenttask(comment)}>
                  <span>{comment.motivation === "editing" ? "Refine suggestion" : "Address comment"}</span><span class="task-chevron" aria-hidden="true">›</span>
                </button>
              </div>
            {/each}
          {/if}
          {#if ondiagnostictask}
            <!-- A compiler's own prose belongs in Diagnostics, not here: a
                 launcher names the task and the place it applies to, so a
                 long or noisy message cannot crowd out the catalog. -->
            {#each diagnostics as item}
              <div class="context-task">
                <p class="panel-muted">{diagnosticLabel(item, path)}</p>
                <button class="task-row" disabled={!caps.can_comment} onclick={() => ondiagnostictask(item)}>
                  <span>Fix diagnostic</span><span class="task-chevron" aria-hidden="true">›</span>
                </button>
              </div>
            {/each}
          {/if}
        </div>
      {/if}
    </div>
  {/if}
  </Tabs.Content>
  </PanelTabs>
  {#if problem || connection.error}<p class="panel-muted" role="alert">{problem || connection.error}</p>{/if}
</section>

<style>
  /* The panel's arrangement -- padding off the panel and onto each pane, the
     strip flush at the top -- is `.panel-tabbed`, shared with the
     collaboration panel. What is left here is this panel's own: its panes
     scroll as a whole, except the chat, whose transcript scrolls under a
     composer that stays put. */
  .agent-panel { gap:calc(var(--spacing) * 3); }
  .agent-panel > :global(*) { flex-shrink:0; }
  .agent-panel :global([role="tabpanel"]) { overflow-y:auto; gap:calc(var(--spacing) * 3); }
  .agent-panel :global(#agent-pane-chat) { overflow:hidden; }
  .agent-panel > :global(p[role="alert"]) { padding-inline:var(--panel-padding); }
  .chat-history { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 3); overflow-y:auto; }
  .chat-history > :global(*) { flex-shrink:0; }
  .agent-panel :global(.agent-tab-content .chat-form) { flex-shrink:0; }
  .agent-setup, .agent-context { display:flex; flex-direction:column; gap:calc(var(--spacing) * 2); }
  .agent-setup p { margin:0; }
  .agent-setup h3, .agent-setup h4 { margin:0; }
  .setup-intro { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5); }
  .setup-title { font-weight:600; }
  .setup-requirement { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5);
                       padding:calc(var(--spacing) * 2); background:var(--color-surface-100-900);
                       border-left:3px solid var(--color-primary-500); }
  .setup-requirement a { font-size:var(--panel-meta-size); }
  /* The install line is long and unbreakable at word boundaries, and the
     panel is a narrow column, so it must be told it may wrap mid-token and
     may not grow past the column. `align-self:flex-start` alone lets it size
     to its content and overflow. */
  .setup-command { align-self:stretch; max-width:100%; padding:calc(var(--spacing) * .5) var(--spacing);
                   background:var(--color-surface-200-800); border-radius:var(--radius-base);
                   user-select:all; overflow-wrap:anywhere; word-break:break-word; }
  .setup-code { max-width:8rem; font-variant-numeric:tabular-nums; letter-spacing:.2em; }
  /* Folded away by default: the port is right for almost everyone, and an
     address field offered up front reads as a decision to make. */
  .setup-address { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5); }
  .setup-address summary { cursor:pointer; }
  .setup-address button { align-self:flex-start; }
  /* Agent and access are two settings, each with one value in force, so each
     is a select. A row of buttons had to signal the chosen one and the signal
     was easy to miss; a select shows its value as its content. */
  .setup-field { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5); min-width:0; }
  .setup-field .select { width:100%; min-width:0; }
  .setup-label { font-size:.7rem; font-weight:600; letter-spacing:.08em; text-transform:uppercase;
                 color:var(--color-surface-600-400); }
  .access-detail { display:flex; flex-direction:column; align-items:flex-start; gap:var(--spacing); }
  .access-detail p { margin:0; }
  .setup-actions, .agent-actions, .attachment-actions { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .agent-actions { justify-content:space-between; }
  .working-dots span { animation:working-dot 1.2s infinite; }
  .working-dots span:nth-child(2) { animation-delay:.2s; }
  .working-dots span:nth-child(3) { animation-delay:.4s; }
  @keyframes working-dot { 0%, 80%, 100% { opacity:.25; } 40% { opacity:1; } }
  @media (prefers-reduced-motion:reduce) { .working-dots span { animation:none; } }
  /* The task catalog is a list, not a set of controls: typography, spacing
     and a hover background carry it, so it stays legible as it grows. */
  .task-list { display:flex; flex-direction:column; }
  .task-group { margin:calc(var(--spacing) * 2) 0 calc(var(--spacing) * .5); font-size:.7rem; font-weight:600;
                letter-spacing:.08em; text-transform:uppercase; color:var(--color-surface-600-400); }
  .task-list > .task-group:first-child { margin-top:0; }
  .task-row { display:flex; align-items:center; justify-content:space-between; gap:var(--spacing);
              width:100%; min-height:2.25rem; padding:calc(var(--spacing) * 1) calc(var(--spacing) * 1.5);
              border:0; border-radius:var(--radius-base, .25rem); background:transparent;
              text-align:left; cursor:pointer; }
  .task-row:hover:not(:disabled), .task-row:focus-visible { background:var(--color-surface-100-900); }
  .task-row:disabled { opacity:.45; cursor:not-allowed; }
  .task-chevron { color:var(--color-surface-600-400); }
  .task-search { width:100%; min-width:0; }
  .context-tasks { display:flex; flex-direction:column; }
  .context-task { display:flex; flex-direction:column; }
  .context-task p { overflow-wrap:anywhere; margin:calc(var(--spacing) * 1.5) 0 0; padding-inline:calc(var(--spacing) * 1.5); font-size:.8rem; }
  .task-back { align-self:flex-start; border:0; background:transparent; padding:0; cursor:pointer; color:var(--color-surface-600-400); }
  .task-back:hover { color:inherit; }
  .task-title { margin:0; font-weight:600; }
  .task-description { margin:calc(var(--spacing) * .5) 0 0; }
  .task-scope .select, .task-prepare .input { width:100%; min-width:0; }
  .prepare-actions { display:flex; justify-content:flex-end; }
  .context-summary { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
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
