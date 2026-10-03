<script>
  import { onMount } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { getPrivate, post, SHELL_HEADERS, keyHeaders } from "../../lib/api.js";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import ChatTranscript from "../ChatTranscript.svelte";
  import ChatComposer from "../ChatComposer.svelte";
  import { createAgentClient, documentKey } from "../../lib/agent-client.svelte.js";
  import * as local from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";
  import {
    captureAttachment, capabilityAllows, diagnosticLabel, suggestionContext,
    normalizeCapabilities, scopeLabel, visibleResults,
    findTask, groupTasks, inferScope, searchTasks, taskPrompt, taskScopes,
  } from "../../lib/assistant.js";
  import { humanizeTool, permissionAction, permissionButtons } from "../../lib/agent-permission.js";

  let {
    slug, link, canShare = false, path = "", selection = null, revision = "", request = null,
    diagnostics = [], oncommenttask, ondiagnostictask,
    comments = [], suggestion: initialSuggestion = null, onreview, onpreview, onsettings,
    userName = "You",
  } = $props();
  let client = $state.raw(null);
  const EMPTY_CONNECTION = { id: "", token: "", messages: [], tasks: {}, connected: false,
    runnerConnected: false, status: "idle", error: "", capabilities: null,
    assistantAgent: "", assistantAccess: "" };
  const connection = $derived(client?.current ?? EMPTY_CONNECTION);
  const connectionId = $derived(connection.id);
  const savedAssistantAgent = $derived(connection.assistantAgent || "");
  const savedAssistantAccess = $derived(connection.assistantAccess || "");
  let busy = $state(false);
  let starting = $state(true);
  let problem = $state("");
  let restoreProblem = $state("");
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
  let restoredConversation = "";
  let chosenAgent = $state("");
  let assistant = $state({ running: false, agent: "", access: "" });
  let pairProblem = $state("");
  // The shared local-app state drives this panel. Watching it reflects probes
  // started elsewhere; the Connect action below is what contacts the app.
  $effect(() => companion.watch());
  const paired = $derived(companion.status?.state === "connected");
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
    { id: "commenter", label: "Comment", help: "Read and comment" },
    { id: "editor", label: "Edit", help: "Apply source changes when asked", warn: true },
  ];
  const currentRole = $derived.by(() => !caps.verified ? "" : caps.can_edit ? "editor" : caps.can_comment ? "commenter" : caps.can_read ? "reader" : "");
  // Without sharing rights the panel can only hand out the access the reader
  // already holds, so the other rows are visible but not selectable.
  const roleOffered = (entry) => canShare || currentRole === entry.id;
  // The last agent and access used in this browser, whatever the document:
  // both nearly always stay put, so neither should be asked for again.
  // Comment is the default because it writes nothing unreviewed.
  const LAST_AGENT = "librepaper.agent.agent";
  const LAST_ACCESS = "librepaper.agent.access";
  // How long a just-started agent may take to join before the send gives up.
  const RUNNER_JOIN_MS = 60_000;
  const AGENT_GRANT_RENEW_MS = 2 * 60_000;
  function lastUsed(key) { try { return localStorage.getItem(key) || ""; } catch { return ""; } }
  function rememberLastUsed(key, value) { try { localStorage.setItem(key, value); } catch { /* private window */ } }

  async function mintAgentGrant(documentLink) {
    const response = await fetch(`/api/documents/${encodeURIComponent(slug)}/agent-token`, {
      method: "POST",
      headers: { ...SHELL_HEADERS, ...keyHeaders(documentKey(documentLink)) },
      credentials: "same-origin", cache: "no-store",
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok || typeof body.token !== "string" || !body.token.startsWith("lpa_")) {
      throw new Error(body.error || "Sign in again to authorize the local assistant.");
    }
    return body.token;
  }
  let access = $state(roles.some((entry) => entry.id === lastUsed(LAST_ACCESS)) ? lastUsed(LAST_ACCESS) : "commenter");
  const runningLabel = $derived(installed.find((entry) => entry.id === assistant.agent)?.label || "assistant");
  let tab = $state("chat");
  const TABS = [
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
  let elapsedTime = $state(0);
  const selectedText = $derived(attachment?.selection?.exact || "");
  const caps = $derived(normalizeCapabilities(verifiedCapabilities));
  const contextPath = $derived(diagnostic?.file || diagnostic?.path || path);
  const preparedTaskValid = $derived(!task || capabilityAllows(caps, task, {
    attached: attachment || null, path: contextPath,
  }));
  const uncertainDelivery = $derived(Object.values(connection.tasks || {}).some((item) =>
    item?.delivery === "uncertain" && item.request === draft.trim()));
  const runnerReady = $derived(connection.connected && connection.runnerConnected);
  const sendable = $derived(runnerReady && preparedTaskValid && !uncertainDelivery);
  // No agent attached yet, but sending can start the chosen one first.
  const canStart = $derived(!runnerReady && !assistant.running && Boolean(connection.id) && Boolean(chosenInstalled?.assistant));
  const startable = $derived(canStart && preparedTaskValid && !uncertainDelivery);
  const taskGroups = $derived(groupTasks(searchTasks(taskQuery)));
  const chosen = $derived(findTask(chosenTaskId));
  const scopeAvailable = (id) => id === "selection" ? Boolean(attachment) : id === "file" ? Boolean(contextPath) : true;
  const allows = (kind, chosenScope) => capabilityAllows(caps, { kind, scope: chosenScope }, {
    attached: attachment || null, path: contextPath,
  });
  // A row is offered when some scope the reader can actually supply works for
  // it; which scope that is belongs to the preparation view, not the catalog.
  const canPrepare = (entry) => taskScopes(entry).some((id) => scopeAvailable(id) && allows(entry.kind, id));
  const preparable = $derived(Boolean(chosen) && scopeAvailable(scope) && allows(chosen.kind, scope));
  const inputTask = $derived(Object.values(connection.tasks || {}).find((item) => item?.status === "needs_input" && item.input) || null);
  // Everything that is currently in flight: what the queued list and status
  // line show, and the composer's Stop button acts on. "Active" (as opposed to
  // queued) is what a Stop button can actually interrupt.
  const activeTasks = $derived(Object.values(connection.tasks || {}).filter((item) =>
    ["queued", "working", "needs_input"].includes(item.status)));
  const activeTask = $derived(activeTasks.find((item) => ["working", "needs_input"].includes(item.status)) || null);
  const queuedTasks = $derived(Object.values(connection.tasks || {}).filter((item) => item?.status === "queued"));

  // The task carries no start time, so the clock starts when this client
  // first sees it working, and restarts for each new working task.
  const workingId = $derived(activeTask?.status === "working" ? activeTask.id : null);
  $effect(() => {
    if (!workingId) return;
    const started = Date.now();
    elapsedTime = 0;
    const timer = setInterval(() => { elapsedTime = Math.floor((Date.now() - started) / 1000); }, 1000);
    return () => clearInterval(timer);
  });

  function formatElapsed(seconds) {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}:${secs.toString().padStart(2, '0')}`;
  }

  // The model the running agent offers to choose, when it offers one. Its pick
  // is remembered per agent and re-applied to each new session of that agent.
  const modelOption = $derived((connection.agentOptions || []).find((option) => option.category === "model") || null);
  const modelName = $derived(modelOption?.choices?.find((choice) => choice.value === modelOption.current)?.name || "");
  const modelKey = () => `librepaper.agent.model.${assistant.agent || chosenAgent}`;
  let modelApplied = "";
  $effect(() => {
    if (!modelOption) { modelApplied = ""; return; }
    const wanted = lastUsed(modelKey());
    const attempt = `${modelOption.id}=${wanted}`;
    if (!wanted || wanted === modelOption.current || modelApplied === attempt
        || !modelOption.choices.some((choice) => choice.value === wanted)) return;
    modelApplied = attempt;
    void client.setOption(modelOption.id, wanted);
  });
  function chooseModel(value) {
    rememberLastUsed(modelKey(), value);
    void client.setOption(modelOption.id, value);
  }

  const progress = $derived(
    activeTask?.status === "working"
      ? `${runningLabel}${modelName ? ` · ${modelName}` : ""} ·${humanizeTool(activeTask.message) || 'Working'}… · ${formatElapsed(elapsedTime)}`
      : activeTask?.status === "needs_input" ? "Waiting for permission" : "");

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

  /// Mint the access link for `mode` and confirm it really grants that
  /// access. The local app is then told this link directly when the
  /// assistant starts, so no connection name or key sits in a config file,
  /// a command line or a chat transcript.
  async function prepareAccessLink(mode) {
    if (canShare) {
      const endpoint = `/api/documents/${encodeURIComponent(slug)}/share`;
      let sharing = await getPrivate(endpoint);
      let accessLink = sharing.links?.[mode];
      if (!accessLink?.key || accessLink.expired) {
        sharing = await post(endpoint, { link: { role: mode, until: "7d", label: "Agent" } });
        accessLink = sharing.links?.[mode];
      }
      if (!accessLink?.url) throw new Error("Could not create an agent access link.");
      agentLink = new URL(accessLink.url, location.origin).href;
    }
    if (!validDocumentLink()) throw new Error("The access link must point to this document.");
    await checkCapabilities(agentLink, { strict: true });
    if (currentRole !== mode) {
      const roleName = mode === "editor" ? "edit" : mode === "commenter" ? "comment" : "read";
      throw new Error(`This access link does not grant ${roleName} access.`);
    }
  }

  async function refreshAgents() {
    if (!paired) { installed = []; return; }
    const answer = await local.agents();
    installed = Array.isArray(answer?.agents) ? answer.agents : [];
    if (!chosenAgent) chosenAgent = installed.find((entry) => entry.assistant && entry.id === lastUsed(LAST_AGENT))?.id
      || installed.find((entry) => entry.assistant)?.id || installed[0]?.id || "";
  }
  $effect(() => { if (paired) void act(refreshAgents); });
  $effect(() => {
    if (!paired || !connectionId) {
      restoreProblem = "";
      return;
    }
    restoreProblem = "";
    const conversationId = connectionId;
    const agent = savedAssistantAgent;
    const savedAccess = savedAssistantAccess;
    let stopped = false;
    let timer;
    let retryDelay = 5_000;
    const restore = async () => {
      const restored = await restoreAssistant(conversationId, agent, savedAccess, () =>
        !stopped && paired && connectionId === conversationId);
      if (restored || stopped || connectionId !== conversationId || !paired) return;
      timer = setTimeout(() => void restore(), retryDelay);
      retryDelay = Math.min(retryDelay * 2, 30_000);
    };
    void restore();
    return () => { stopped = true; clearTimeout(timer); };
  });
  $effect(() => {
    if (!paired || !assistant.running || !connectionId) return;
    const conversationId = connectionId;
    const target = agentLink || link;
    let stopped = false;
    const renew = async () => {
      try {
        const agentToken = await mintAgentGrant(target);
        if (!stopped) await local.renewAssistant({ link: target, conversation: conversationId, agentToken });
      } catch (error) {
        if (!stopped) problem = error?.message || String(error);
      }
    };
    const timer = setInterval(() => void renew(), AGENT_GRANT_RENEW_MS);
    return () => { stopped = true; clearInterval(timer); };
  });

  /// The explicit Connect action refreshes status first: restoring a saved
  /// Agent panel is not a request to contact the companion. Only start pairing
  /// if this click finds it is not already connected.
  async function connectToApp() {
    pairProblem = "";
    busy = true;
    try {
      const status = await local.retry();
      if (status.state !== "connected") await local.connectApp();
      await refreshAgents();
    } catch (error) {
      pairProblem = error?.message || String(error);
    } finally {
      busy = false;
    }
  }

  /// Start the assistant: mint the access link and have the local app drive
  /// the chosen agent directly against it. Sending the first message is what
  /// calls this, so it throws for the caller to report.
  async function startAssistant(mode) {
    // One assistant per conversation. Starting a second against the same
    // one would leave the first holding the link it was started with, so a
    // change of agent or access replaces it rather than racing it.
    if (assistant.running) {
      await local.stopAssistant({ link: agentLink || link, conversation: connection.id });
      assistant = { running: false, agent: "", access: "" };
    }
    await prepareAccessLink(mode);
    const agentToken = await mintAgentGrant(agentLink || link);
    const answer = await local.startAssistant({
      link: agentLink || link, conversation: connection.id,
      chatToken: connection.token, agentToken, agent: chosenAgent,
    });
    if (!answer?.running) throw new Error(answer?.error || "The assistant did not start.");
    assistant = { running: true, agent: chosenAgent, access: mode };
    client.rememberAssistant({ agent: chosenAgent, access: mode });
    rememberLastUsed(LAST_AGENT, chosenAgent);
    rememberLastUsed(LAST_ACCESS, mode);
  }

  /// A reader who cannot share can only hand the agent the access they hold,
  /// whatever was last used elsewhere.
  function offeredAccess() {
    return roleOffered({ id: access }) || !currentRole ? access : currentRole;
  }

  /// Change the agent or the access level. A running assistant holds the link
  /// it was started with, so a change restarts it rather than leaving the
  /// selection claiming something that is not in force. That includes a runner
  /// attached before a reload, whose access this panel cannot know.
  async function choose(change) {
    const before = `${chosenAgent} ${access}`;
    change();
    if (`${chosenAgent} ${access}` === before) return;
    if (assistant.running || runnerReady) await act(() => startAssistant(offeredAccess()));
  }

  /// The local app's status call reports only whether an assistant is
  /// running; which agent and access level that is comes back from the
  /// per-conversation record this browser already remembered.
  async function restoreAssistant(conversationId, agent, savedAccess, isCurrent) {
    if (restoredConversation === conversationId) return true;
    if (!agent && !savedAccess) {
      restoredConversation = conversationId;
      restoreProblem = "";
      return true;
    }
    try {
      // Re-fetch the same named link the runner was started with. This keeps
      // the browser able to renew that runner's grant after a reload, without
      // persisting the link key in its agent-session record.
      if (savedAccess) await prepareAccessLink(savedAccess);
      if (!isCurrent()) return true;
      const answer = await local.assistantStatus({ link: agentLink || link, conversation: conversationId });
      if (!isCurrent()) return true;
      // A successful status response saying the runner is stopped is terminal
      // for this restore. Network or server errors below remain retryable.
      if (!answer?.running) {
        restoredConversation = conversationId;
        restoreProblem = "";
        return true;
      }
      assistant = { running: true, agent, access: savedAccess };
      if (savedAccess) access = savedAccess;
      if (agent) chosenAgent = agent;
      restoredConversation = conversationId;
      restoreProblem = "";
      return true;
    } catch (error) {
      // A one-off companion or server failure must not disable renewal for a
      // runner that is still active. The effect retries with bounded backoff.
      if (!isCurrent()) return true;
      restoreProblem = error?.message || String(error);
      return false;
    }
  }

  async function act(operation) {
    busy = true; problem = "";
    try { await operation(); }
    catch (error) { problem = error?.message || String(error); }
    finally { busy = false; }
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
    draft = "";
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
      if (!next.comment && !next.diagnostic) {
        const label = task.kind.charAt(0).toUpperCase() + task.kind.slice(1);
        draft = taskPrompt({ label }, task.scope);
      }
    }
    if (next.comment) {
      const quoted = next.comment.source || next.comment;
      if (quoted?.exact) attach({ ...quoted, revision: next.revision || next.comment.revision || revision }, next.revision || next.comment.revision || revision);
      else { suppressedSelection = selectionKey(selection || lastSelection); attachment = null; }
      scope = attachment ? "selection" : (contextPath ? "file" : "document");
      task = { kind: next.comment.suggestion ? "refine" : "respond", scope };
      draft = next.comment.suggestion ? "Refine this suggestion." : "Address this comment.";
    }
    if (next.diagnostic) {
      task = next.task || { kind: "fix", scope };
      if (!draft.trim()) draft = task.kind === "fix" ? "Fix this diagnostic." : "Explain this diagnostic.";
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

  /// Picking a task is the decision; writing a draft the reader then has to
  /// send again was a second confirmation of the same thing. This sends it and
  /// moves to the chat, where the answer will arrive.
  async function sendTask() {
    // Deliberately not gated on `sendable`: that judges the task currently in
    // context, which is the previous one. What matters is whether the task
    // being sent is allowed, which is `preparable`, and whether the assistant
    // is there or can be started. `send` makes the final check once this task
    // is in context.
    if (!chosen || !preparable || !(runnerReady || canStart)) return;
    const entry = chosen;
    task = { kind: entry.kind, scope };
    const note = extra.trim();
    const prompt = taskPrompt(entry, scope);
    if (entry.kind !== "explain") { diagnostic = null; diagnosticRevision = ""; }
    // Leave the catalog showing for the next request; "Change context" in the
    // chat pane reopens this task's view.
    taskView = "launcher";
    tab = "chat";
    draft = "";
    await send(note ? `${prompt}\n\n${note}` : prompt);
  }

  // Scope is chosen inside the preparation view, before anything is written
  // into the draft, so changing it neither rewrites the composer nor discards
  // the attached passage the user may switch back to.
  function chooseScope(next) {
    scope = next;
  }

  async function send(text) {
    if (busy || !(sendable || startable)) return false;
    let sent = false;
    await act(async () => {
      if (!runnerReady) {
        await startAssistant(offeredAccess());
        await client.untilRunner(RUNNER_JOIN_MS);
      }
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

  async function newConversation() {
    await act(async () => {
      if (assistant.running) await local.stopAssistant({ link: agentLink || link, conversation: connection.id });
      assistant = { running: false, agent: "", access: "" };
      await client.end(); await client.create();
      draft = ""; task = null; attachment = null; suggestion = null; commentContext = null;
      pendingRequest = null; diagnostic = null; diagnosticRevision = "";
      suppressedSelection = selectionKey(selection || lastSelection);
      tab = "chat";
    });
  }

  async function reconnect() { await act(() => client.reconnect()); }

  function uncertainTasks() { return Object.values(connection.tasks || {}).filter((item) => item?.delivery === "uncertain"); }
  async function retryTask(id) {
    await act(async () => {
      if (!await client.retry(id)) throw new Error("The original request is no longer available to retry.");
    });
  }
  /// Answer a permission request with one of the options the agent offered.
  /// The runner rejects anything else, so the two sides cannot drift into
  /// answering a different question.
  async function answerInput(option) {
    const item = inputTask;
    if (!item?.input?.request_id) return;
    await act(() => client.respond(item.id, item.input.request_id, { option }));
  }

  function chooseResult(results) {
    const reviewable = visibleResults({ context: { results } }, comments);
    if (reviewable.suggestions.length) onreview?.(reviewable);
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
  async function checkCapabilities(nextLink = agentLink, { strict = false } = {}) {
    const generation = ++capabilityGeneration;
    verifiedCapabilities = null;
    // The browser keeps its own access link; the setup prompt grants the agent a separate role.
    try {
      const result = await client?.capabilities(nextLink);
      if (generation !== capabilityGeneration) return null;
      verifiedCapabilities = result;
      return result;
    } catch (error) {
      if (generation !== capabilityGeneration) return null;
      verifiedCapabilities = null;
      if (strict) throw error;
    }
    return null;
  }

  onMount(() => {
    agentLink = link || "";
    client = createAgentClient({ origin: location.origin, slug, link: agentLink || link, onpreview });
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
  <!-- This stays in the local template scope instead of becoming an unknown
       snippet prop on PanelTabs. -->
  {#snippet connectPrompt()}
    <div class="connect-required">
      <p class="panel-muted">Connect the LibrePaper app on this computer to use an agent.</p>
      <div class="setup-actions">
        <button class="btn btn-sm lp-control-brand" disabled={busy}
                onclick={() => void connectToApp()}>{busy ? "Connecting…" : "Connect"}</button>
      </div>
      <!-- A refused connection must say so here, beside the button that was
           pressed, rather than in the panel's shared error line below. -->
      {#if pairProblem}<span class="panel-meta" role="alert">{pairProblem}</span>{/if}
    </div>
  {/snippet}
  <!-- The strip, its ids and the layout of a pane are PanelTabs', shared with
       the collaboration panel. What is left here is what the tabs contain. -->
  <PanelTabs id="agent" label="Agent" listClass="agent-tabs" tabs={TABS}
             value={tab} onchange={(value) => tab = value}>

  <!-- The one manual step, done once per computer rather than once per
       document. It is the same pairing the local compiler uses, so a
       reader who already paired for Quarto or native TeX skips it. Until it
       is done there is nothing else to offer: no port, no install command,
       just the one button that starts it. -->
  <!-- The gear at the top of each pane is the one way to Local companion
       settings now; the Chat pane also gets Clear conversation beside it. -->
  {#snippet paneHeader(withNewConversation)}
    <div class="pane-header">
      {#if withNewConversation && connection.id}
        <IconButton icon="eraser" label="Clear conversation" tone="plain" size="btn-icon-sm"
                    disabled={busy} onclick={() => void newConversation()} />
      {/if}
      <IconButton icon="sliders" label="Local companion settings" tone="plain" size="btn-icon-sm"
                  onclick={() => onsettings?.()} />
    </div>
  {/snippet}

  <!-- The permission card sits inside the transcript, through ChatTranscript's
       `after` snippet, so it reads as part of the conversation rather than a
       dashboard bolted underneath it. -->
  {#snippet permissionCard()}
    {#if inputTask}
      <div class="permission-card" role="group" aria-label="Assistant input request">
        <span class="permission-label">Permission required</span>
        <p>{permissionAction(inputTask.input)}</p>
        <!-- The agent names its own options, so the card renders exactly
             those, ordered and labelled by kind. Inventing an option it
             never offered would be answering a question it did not ask. -->
        <div class="setup-actions">
          {#each permissionButtons(inputTask.input.options) as option (option.id)}
            <button class="btn btn-sm {option.primary ? 'lp-control-brand' : ''}" disabled={busy}
                    onclick={() => void answerInput(option.id)}>{option.label}</button>
          {/each}
        </div>
        {#if inputTask.input.details}
          {@const details = inputTask.input.details}
          <details class="permission-info">
            <summary>Details</summary>
            <div class="permission-details">
              {#if details.command}<p><strong>Command</strong><code>{details.command.slice(0, 2048)}{#if details.command.length > 2048}…{/if}</code></p>{/if}
              {#if details.files?.length}<p><strong>Files</strong><span>{details.files.slice(0, 8).map((path) => String(path).slice(0, 512)).join(", ")}{#if details.files.length > 8} and {details.files.length - 8} more{/if}</span></p>{/if}
              {#if details.diff}<details><summary>Review changes</summary><pre>{details.diff.slice(0, 2048)}{#if details.diff.length > 2048}…{/if}</pre></details>{/if}
            </div>
          </details>
        {/if}
      </div>
    {/if}
  {/snippet}

  <!-- What the agent is doing, as the last line of the conversation rather than
       beside the buttons: it changes length as the work goes on, and the
       composer must not move when it does. -->
  {#snippet transcriptEnd()}
    {@render permissionCard()}
    {#if progress}<p class="agent-progress panel-meta" role="status">{progress}</p>{/if}
  {/snippet}

  <Tabs.Content value="chat" class="agent-tab-content">
  {@render paneHeader(true)}
  {#if !paired}
    {@render connectPrompt()}
  {:else}
    <div class="chat-history">
    {#if !connection.id}<button class="btn lp-control-brand" disabled={busy || starting} onclick={() => void act(() => client.create())}>{starting ? "Connecting…" : "Retry connection"}</button>{/if}
    {#if connection.id && !connection.connected}
      <div role="status"><span class="panel-muted">Reconnecting…</span> <button class="btn btn-sm lp-control-outline" disabled={busy} onclick={() => void reconnect()}>Reconnect now</button></div>
    {/if}

  {#if pendingRequest}
    <div class="request-warning" role="alert">
      <span>A new assistant request is ready. Replace the current draft and attached context?</span>
      <div class="setup-actions"><button class="btn btn-sm lp-control-brand" onclick={() => applyRequest(pendingRequest)}>Replace draft and context</button><button class="btn btn-sm" onclick={() => pendingRequest = null}>Keep current draft</button></div>
    </div>
  {/if}

  {#each uncertainTasks() as item (item.id)}
    <p class="panel-meta" role="alert">Message delivery was not confirmed. <button class="btn btn-sm lp-control-outline" disabled={busy} onclick={() => void retryTask(item.id)}>Retry delivery</button></p>
  {/each}
  <ChatTranscript messages={connection.messages}
                  empty={connection.runnerConnected ? "No messages yet." : "Send a message to start the agent."}
                  authors={false} quiet onresult={chooseResult} after={transcriptEnd}
                  roleLabel={(message) => message.role === "user" ? userName : (message.creator || message.role)} />

  <!-- The draft carries its own context, so the chat pane states it in one
       line and sends the user back to the task view to change it. -->
  {#if task || attachment}<p class="panel-meta context-summary">{scopeLabel(scope, { path: contextPath, attached: attachment })} <button class="btn btn-sm" onclick={() => { tab = "tasks"; taskView = chosen ? "prepare" : "launcher"; }}>Change context</button>{#if attachment}<button class="btn btn-sm" onclick={removeSelection}>Remove passage</button>{/if}</p>{/if}
  <!-- Missing access or context is worth explaining; a runner that simply
       is not there yet starts when the message is sent, so that case gets no
       line here. -->
  {#if !sendable && (uncertainDelivery || connection.runnerConnected)}
    <p class="panel-meta">{uncertainDelivery ? "This request was not confirmed. Retry delivery above before sending it again." : "This task needs the appropriate access and context. Choose another task or attach a passage."}</p>
  {/if}
    </div>
  {#if queuedTasks.length}
  <div class="queued-requests">
  {#each queuedTasks as item (item.id)}
    <div class="queued-request" data-task-id={item.id}>
      <span class="queued-label">Queued: {item.request || "Request"}</span>
      {#if item.cancelRequested}
        <span class="queued-status">Cancellation requested</span>
      {:else}
        <button class="btn btn-sm" disabled={busy || !runnerReady} onclick={() => void act(() => client.cancel(item.id))}>Cancel</button>
      {/if}
    </div>
  {/each}
  </div>
  {/if}
  <!-- Which agent, with what access: one quiet line, since both nearly always
       stay as they were last time. Sending starts the agent; changing either
       while it runs restarts it; the model, when the agent offers one,
       changes in place. -->
  <div class="agent-settings">
    {#if installed.length}
      <select class="agent-setting" aria-label="Agent" disabled={busy} value={chosenAgent}
              onchange={(event) => void choose(() => chosenAgent = event.currentTarget.value)}>
        {#each installed as entry (entry.id)}<option value={entry.id}>{entry.label}</option>{/each}
      </select>
      <span aria-hidden="true">·</span>
      <select class="agent-setting" aria-label="Access" disabled={busy || !connection.id} value={access}
              onchange={(event) => void choose(() => access = event.currentTarget.value)}>
        {#each roles as entry}<option value={entry.id} disabled={!roleOffered(entry)} title={entry.help}>{entry.label}</option>{/each}
      </select>
      {#if modelOption}
        <span aria-hidden="true">·</span>
        <select class="agent-setting" aria-label="Model" disabled={busy} value={modelOption.current}
                onchange={(event) => chooseModel(event.currentTarget.value)}>
          {#each modelOption.choices as choice (choice.value)}<option value={choice.value}>{choice.name}</option>{/each}
        </select>
      {/if}
    {:else}
      <span>No supported coding agent was found on this computer.</span>
    {/if}
  </div>
  <!-- Why sending cannot start this agent, or what its route cannot do. -->
  {#if chosenInstalled?.assistant_blocked || chosenInstalled?.assistant_note}
    <p class="panel-meta" data-agent-note={chosenInstalled.id}>{chosenInstalled.assistant_blocked}{chosenInstalled.assistant_note}</p>
  {/if}
  {#if assistant.agent !== chosenAgent && chosenInstalled?.assistant_fetches}
    <p class="panel-meta" data-adapter-fetch>Starting it downloads its adapter the first time.</p>
  {/if}
  <ChatComposer placeholder="Ask your agent…" canSend={!busy && (sendable || startable)} draft={draft} ondraft={(value) => draft = value} onsend={send}
                onstop={assistant.running && activeTask ? () => void act(() => client.cancel(activeTask.id)) : null}
                stopLabel="Stop" />
  {/if}
  </Tabs.Content>
  <Tabs.Content value="tasks" class="agent-tab-content">
  {@render paneHeader(false)}
  {#if !paired}
    {@render connectPrompt()}
  {:else}
  {#if taskView === "prepare" && chosen}
    <div class="agent-context task-prepare">
      <button class="task-back" onclick={() => taskView = "launcher"}>← All tasks</button>
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
      {:else if !runnerReady}<p class="panel-meta" role="status">Start an agent in the Chat tab before sending a task.</p>{/if}
      <div class="prepare-actions"><button class="btn btn-sm lp-control-brand" disabled={busy || !preparable || !(runnerReady || canStart)} onclick={() => void sendTask()}>Send to agent</button></div>
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
  {/if}
  </Tabs.Content>
  </PanelTabs>
  {#if problem || restoreProblem || connection.error}<p class="panel-muted" role="alert">{problem || restoreProblem || connection.error}</p>{/if}
</section>

<style>
  /* The panel's arrangement -- padding off the panel and onto each pane, the
     strip flush at the top -- is `.panel-tabbed`, shared with the
     collaboration panel. What is left here is this panel's own: its panes
     scroll as a whole, except the chat, whose transcript scrolls under a
     composer that stays put. */
  .agent-panel { gap:calc(var(--spacing) * 2); }
  .agent-panel > :global(*) { flex-shrink:0; }
  .agent-panel :global([role="tabpanel"]) { overflow-y:auto; gap:calc(var(--spacing) * 2); }
  .agent-panel :global(#agent-pane-chat) { overflow:hidden; }
  .agent-panel > :global(p[role="alert"]) { padding-inline:var(--panel-padding); }
  /* The gear (and, in Chat, Clear conversation) sits above everything else in
     its pane, aligned right: it is upkeep, not the point of the pane. */
  .pane-header { display:flex; justify-content:flex-end; gap:calc(var(--spacing) * .5); }
  .chat-history { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 2); overflow-y:auto; }
  .chat-history > :global(*) { flex-shrink:0; }
  .agent-panel :global(.agent-tab-content .chat-form) { flex-shrink:0; }
  .agent-context { display:flex; flex-direction:column; gap:calc(var(--spacing) * 2); }
  .agent-settings { display:flex; align-items:center; gap:calc(var(--spacing) * .5); font-size:.8em; color:var(--color-text-secondary); }
  .agent-setting { border:0; background:transparent; color:inherit; font:inherit; padding:0; cursor:pointer; max-width:12rem; }
  /* The whole not-paired warning: one line, the Connect button, and the
     refusal message if there is one. */
  .connect-required { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5);
                       padding:calc(var(--spacing) * 2); background:var(--color-subtle);
                       border-left:3px solid var(--color-brand); }
  .connect-required p { margin:0; }
  /* Agent and access are two settings, each with one value in force, so each
     is a select. A row of buttons had to signal the chosen one and the signal
     was easy to miss; a select shows its value as its content. */
  .setup-actions, .attachment-actions { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  /* Queued requests: a compact list above the composer showing what is queued,
     with a cancel button for each. Secondary text colour keeps it quiet. */
  .queued-request { display:flex; align-items:center; justify-content:space-between; gap:var(--spacing);
                    padding:calc(var(--spacing) * .75) calc(var(--spacing) * 1.5); font-size:.85em; }
  /* One line each and a bounded list, so a backlog never pushes the composer
     out of a short pane. */
  .agent-progress { margin:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .queued-requests { flex:none; max-height:2.25rem; overflow-y:auto; }
  .queued-label { color:var(--color-text-secondary); min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .queued-status { color:var(--color-text-secondary); font-size:.8em; }
  /* The task catalog is a list, not a set of controls: typography, spacing
     and a hover background carry it, so it stays legible as it grows. */
  .task-list { display:flex; flex-direction:column; }
  .task-group { margin:calc(var(--spacing) * 2) 0 calc(var(--spacing) * .5); font-size:.7rem; font-weight:600;
                letter-spacing:.08em; text-transform:uppercase; color:var(--color-text-secondary); }
  .task-list > .task-group:first-child { margin-top:0; }
  .task-row { display:flex; align-items:center; justify-content:space-between; gap:var(--spacing);
              width:100%; min-height:2.25rem; padding:calc(var(--spacing) * 1) calc(var(--spacing) * 1.5);
              border:0; border-radius:var(--radius-base, .25rem); background:transparent;
              text-align:left; cursor:pointer; }
  .task-row:hover:not(:disabled), .task-row:focus-visible { background:var(--color-subtle); }
  .task-row:disabled { opacity:.45; cursor:not-allowed; }
  .task-chevron { color:var(--color-text-secondary); }
  .task-search { width:100%; min-width:0; }
  .context-tasks { display:flex; flex-direction:column; }
  .context-task { display:flex; flex-direction:column; }
  .context-task p { overflow-wrap:anywhere; margin:calc(var(--spacing) * 1.5) 0 0; padding-inline:calc(var(--spacing) * 1.5); font-size:.8rem; }
  .task-back { align-self:flex-start; border:0; background:transparent; padding:0; cursor:pointer; color:var(--color-text-secondary); }
  .task-back:hover { color:inherit; }
  .task-title { margin:0; font-weight:600; }
  .task-description { margin:calc(var(--spacing) * .5) 0 0; }
  .task-scope .select, .task-prepare .input { width:100%; min-width:0; }
  .prepare-actions { display:flex; justify-content:flex-end; }
  .context-summary { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .attachment { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); border-left:3px solid var(--color-brand); background:var(--color-subtle); }
  .attachment blockquote { max-height:7rem; overflow:auto; margin:var(--spacing) 0 0; white-space:pre-wrap; overflow-wrap:anywhere; }
  .diagnostic-context { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); background:var(--color-subtle); }
  .diagnostic-context span { white-space:pre-wrap; overflow-wrap:anywhere; }
  .request-warning { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 2); background:var(--color-warning-bg); overflow-wrap:anywhere; }
  /* A compact card inside the transcript rather than a dashboard block below
     it: a thin accent border says "this needs you" without a heavy frame. */
  .permission-card { display:flex; flex-direction:column; gap:calc(var(--spacing) * .75); padding:calc(var(--spacing) * 1.5); border-left:3px solid var(--color-warning-solid); background:var(--color-subtle); }
  .permission-card p { margin:0; white-space:pre-wrap; overflow-wrap:anywhere; }
  .permission-label { font-size:.7rem; font-weight:600; letter-spacing:.08em; text-transform:uppercase; color:var(--color-text-secondary); }
  .permission-info summary { cursor:pointer; font-size:.8em; color:var(--color-text-secondary); }
  .permission-details { display:flex; flex-direction:column; gap:var(--spacing); overflow:hidden; margin-top:var(--spacing); }
  .permission-details p { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5); }
  .permission-details code, .permission-details pre { max-height:14rem; overflow:auto; white-space:pre-wrap; overflow-wrap:anywhere; }
  .permission-details pre { margin:0; padding:var(--spacing); background:var(--color-divider); }
  .agent-panel :global(.chat-transcript-wrap) { flex:1 0 8rem; min-height:8rem; }
  @media (max-height:600px) { .attachment blockquote { max-height:4rem; } }
</style>
