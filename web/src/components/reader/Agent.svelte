<script>
  import { onMount, tick } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { getPrivate, post, SHELL_HEADERS, keyHeaders } from "../../lib/api.js";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import ChatTranscript from "../ChatTranscript.svelte";
  import ChatComposer from "../ChatComposer.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import { createAgentClient, documentKey } from "../../lib/agent-client.svelte.js";
  import * as local from "../../lib/desktop/client.js";
  import {
    captureAttachment, capabilityAllows, diagnosticLabel, suggestionContext,
    normalizeCapabilities, scopeLabel, visibleResults,
    TASK_CATEGORIES, browseTasks, findTask, inferScope, recentTasks, rememberTask,
    searchTasks, taskBlocker, taskForKind, taskPrompt, taskScopes,
  } from "../../lib/assistant.js";
  import { humanizeTool, permissionAction, permissionButtons } from "../../lib/agent-permission.js";

  let {
    slug, link, canShare = false, path = "", selection = null, revision = "", request = null,
    diagnostics = [],
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
  // The Tasks pane is a launcher: a short list of recent tasks or search
  // results, then a preparation view for the one task the user picked. The
  // launcher's query, category, highlight and scroll position survive a visit
  // to that view.
  let taskView = $state("launcher");
  let taskQuery = $state("");
  let browsing = $state(false);
  let category = $state("");
  let highlight = $state(0);
  let chosenTaskId = $state("");
  let extra = $state("");
  let listEl = $state(null);
  let searchEl = $state(null);
  let backEl = $state(null);
  let savedScroll = 0;
  let agentLink = $state("");
  // What this computer has, as the local app reports it. The browser cannot
  // read a PATH, so without the desktop app there is nothing to offer and the
  // panel says so rather than guessing.
  let installed = $state([]);
  let restoredConversation = "";
  let chosenAgent = $state("");
  let assistant = $state({ running: false, agent: "", access: "" });
  const paired = Boolean(local.desktop());
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
  const RECENT_TASKS = "librepaper.agent.recentTasks";
  function lastUsed(key) { try { return localStorage.getItem(key) || ""; } catch { return ""; } }
  function rememberLastUsed(key, value) { try { localStorage.setItem(key, value); } catch { /* private window */ } }
  function storedTaskIds() {
    try {
      const ids = JSON.parse(lastUsed(RECENT_TASKS) || "[]");
      return Array.isArray(ids) ? ids.filter((id) => typeof id === "string") : [];
    } catch { return []; }
  }
  let recentIds = $state(storedTaskIds());

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
  const launcherRows = $derived.by(() => {
    const query = taskQuery.trim();
    if (query) {
      const found = searchTasks(query);
      return browsing && category ? found.filter((entry) => entry.category === category) : found;
    }
    return browsing ? browseTasks(category) : recentTasks(recentIds);
  });
  const chosen = $derived(findTask(chosenTaskId));
  const COMMENT_TASKS = ["address-comment", "refine-suggestion"];
  const contextTask = $derived(COMMENT_TASKS.includes(chosenTaskId) || chosenTaskId === "fix-diagnostic");
  const scopeAvailable = (id) => id === "selection" ? Boolean(attachment) : id === "file" ? Boolean(contextPath) : true;
  // Rows are never disabled; what stops a task is said in the preparation view.
  const staleReason = $derived.by(() => {
    if (!chosen) return "";
    if (COMMENT_TASKS.includes(chosenTaskId) && commentContext) {
      const live = comments.find((item) => String(item.id) === String(commentContext.id));
      if (!live || live.resolved) return "The attached comment is no longer available.";
    }
    if (diagnostic) {
      const file = diagnostic.file || diagnostic.path || "";
      const present = diagnostics.some((item) => (item.file || item.path || "") === file
        && (item.line || 0) === (diagnostic.line || 0) && item.message === diagnostic.message);
      if (!present) return "This diagnostic is gone after the last compile.";
    }
    return "";
  });
  const blocker = $derived(chosen ? taskBlocker(caps, { kind: chosen.kind, scope }, {
    attached: attachment || null, path: contextPath,
  }) : "");
  const agentReady = $derived(runnerReady || canStart);
  const commentTarget = $derived(COMMENT_TASKS.includes(chosenTaskId) && commentContext
    ? `Comment: "${clip(commentContext.body || commentContext.exact || "", 80)}"` : "");
  const diagnosticTarget = $derived(diagnostic
    ? (diagnostic.message
      ? `${contextPath}${diagnostic.line ? `:${diagnostic.line}` : ""} ${diagnostic.message}`.trim()
      : diagnosticLabel(diagnostic, path))
    : "");
  const selectedWords = $derived(selectedText.trim().split(/\s+/).filter(Boolean).length);
  function clip(text, max) { return text.length > max ? `${text.slice(0, max)}...` : text; }
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
      // A one-off desktop app or server failure must not disable renewal for a
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
    tab = "tasks";
    extra = "";
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
    }
    if (next.comment) {
      const quoted = next.comment.source || next.comment;
      if (quoted?.exact) attach({ ...quoted, revision: next.revision || next.comment.revision || revision }, next.revision || next.comment.revision || revision);
      else { suppressedSelection = selectionKey(selection || lastSelection); attachment = null; }
      scope = attachment ? "selection" : (contextPath ? "file" : "document");
      task = { kind: next.comment.suggestion ? "refine" : "respond", scope };
    }
    if (next.diagnostic) task = next.task || { kind: "fix", scope };
    chosenTaskId = next.comment ? (next.comment.suggestion ? "refine-suggestion" : "address-comment")
      : next.diagnostic ? (next.task?.kind === "explain" ? taskForKind("explain")?.id || "" : "fix-diagnostic")
      : next.task ? taskForKind(next.task.kind)?.id || "" : "";
    lastRequestId = next.id || lastRequestId;
    if (chosenTaskId) enterPrepare();
    else taskView = "launcher";
  }

  function requestArrived(next) {
    if (!next?.id || next.id === lastRequestId) return;
    lastRequestId = next.id;
    tab = "tasks";
    if (taskView === "prepare" && extra.trim()) pendingRequest = next;
    else applyRequest(next);
  }

  function enterPrepare() {
    if (taskView !== "prepare") savedScroll = listEl?.scrollTop || 0;
    taskView = "prepare";
    void tick().then(() => backEl?.focus());
  }

  function leavePrepare({ focus = true } = {}) {
    taskView = "launcher";
    void tick().then(() => {
      if (listEl) listEl.scrollTop = savedScroll;
      if (focus) searchEl?.focus();
    });
  }

  function openTask(entry) {
    if (!entry) return;
    chosenTaskId = entry.id;
    extra = "";
    scope = inferScope(entry, { attached: attachment, path: contextPath });
    enterPrepare();
  }

  function startBrowse() {
    browsing = true; category = ""; highlight = 0;
    searchEl?.focus();
  }

  function stopBrowse() {
    browsing = false; category = ""; highlight = 0;
    searchEl?.focus();
  }

  function scrollToHighlight() {
    void tick().then(() => listEl?.querySelector(".task-row.active")?.scrollIntoView({ block: "nearest" }));
  }

  function searchKeydown(event) {
    if (event.isComposing) return;
    const rows = launcherRows;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (!rows.length) return;
      highlight = (highlight + (event.key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length;
      event.preventDefault();
      scrollToHighlight();
    } else if ((event.key === "Home" || event.key === "End") && taskQuery && rows.length) {
      highlight = event.key === "Home" ? 0 : rows.length - 1;
      event.preventDefault();
      scrollToHighlight();
    } else if (event.key === "Enter") {
      if (!rows[highlight]) return;
      event.preventDefault();
      openTask(rows[highlight]);
    } else if (event.key === "Escape") {
      if (taskQuery) { taskQuery = ""; highlight = 0; event.preventDefault(); }
      else if (browsing) { stopBrowse(); event.preventDefault(); }
    }
  }

  function prepareKeydown(event) {
    if (event.key !== "Escape" || event.target.tagName === "SELECT" || extra) return;
    event.preventDefault();
    leavePrepare();
  }

  /// Picking a task and confirming it is the one decision. A send that fails
  /// leaves the preparation view as it was so the user can retry.
  async function sendTask() {
    if (busy || !chosen || staleReason || blocker || !agentReady) return;
    const entry = chosen;
    const fromContext = contextTask;
    task = { kind: entry.kind, scope };
    const note = extra.trim();
    const prompt = taskPrompt(entry, scope);
    if (!fromContext && entry.kind !== "explain") { diagnostic = null; diagnosticRevision = ""; }
    if (!await send(note ? `${prompt}\n\n${note}` : prompt)) return;
    if (!fromContext) {
      recentIds = rememberTask(recentIds, entry.id);
      try { localStorage.setItem(RECENT_TASKS, JSON.stringify(recentIds)); } catch { /* private window */ }
    }
    extra = "";
    tab = "chat";
    leavePrepare({ focus: false });
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
      <DesktopOnly feature="The agent" description="Chat with Claude, Codex or another coding agent that reads and edits this project." />
    </div>
  {/snippet}
  <!-- The gear at the top of each pane is the one way to the desktop app
       settings now; the Chat pane also gets Clear conversation beside it. -->
  {#snippet paneHeader(withNewConversation)}
    <div class="pane-header">
      {#if withNewConversation && connection.id}
        <IconButton icon="eraser" label="Clear conversation" tone="plain" size="btn-icon-sm"
                    disabled={busy} onclick={() => void newConversation()} />
      {/if}
      <IconButton icon="sliders" label="Desktop app settings" tone="plain" size="btn-icon-sm"
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

  <!-- The strip, its ids and the layout of a pane are PanelTabs', shared with
       the collaboration panel. What is left here is what the tabs contain. -->
  <PanelTabs id="agent" label="Agent" listClass="agent-tabs" tabs={TABS}
             value={tab} onchange={(value) => tab = value}>

  <Tabs.Content value="chat" class="agent-tab-content">
  {@render paneHeader(true)}
  {#if !paired}
    {@render connectPrompt()}
  {:else}
    <div class="chat-history">
    {#if !connection.id}<button class="btn lp-control-brand" disabled={busy || starting} onclick={() => void act(() => client.create())}>{starting ? "Connecting…" : "Retry connection"}</button>{/if}
    {#if connection.id && !connection.connected}
      <div role="status"><span class="panel-muted">Reconnecting…</span> <button class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => void reconnect()}>Reconnect now</button></div>
    {/if}

  {#each uncertainTasks() as item (item.id)}
    <p class="panel-meta" role="alert">Message delivery was not confirmed. <button class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => void retryTask(item.id)}>Retry delivery</button></p>
  {/each}
  <ChatTranscript messages={connection.messages}
                  empty={connection.runnerConnected ? "No messages yet." : "Send a message to start the agent."}
                  authors={false} quiet onresult={chooseResult} after={transcriptEnd}
                  roleLabel={(message) => message.role === "user" ? userName : (message.creator || message.role)} />

  <!-- The draft carries its own context, so the chat pane states it in one
       line and sends the user back to the task view to change it. -->
  {#if task || attachment}<p class="panel-meta context-summary">{scopeLabel(scope, { path: contextPath, attached: attachment })} <button class="btn btn-sm" onclick={() => { tab = "tasks"; if (chosen) enterPrepare(); else taskView = "launcher"; }}>Change context</button>{#if attachment}<button class="btn btn-sm" onclick={removeSelection}>Remove passage</button>{/if}</p>{/if}
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
  {#if pendingRequest}
    <div class="request-warning" role="alert">
      <span>A new request arrived. Replace this one?</span>
      <div class="setup-actions"><button class="btn btn-sm lp-control-brand" onclick={() => applyRequest(pendingRequest)}>Replace</button><button class="btn btn-sm" onclick={() => pendingRequest = null}>Keep current</button></div>
    </div>
  {/if}
  {#if taskView === "prepare" && chosen}
    <!-- Escape is a convenience on top of the Back button. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="agent-context task-prepare" onkeydown={prepareKeydown}>
      <div class="task-head">
        <button class="task-back" aria-label="Back to tasks" bind:this={backEl} onclick={() => leavePrepare()}>←</button>
        <h3 class="task-title">{chosen.label}</h3>
      </div>
      <p class="panel-muted task-description">{chosen.description}</p>
      {#if commentTarget}<p class="panel-meta task-target" title={commentContext.body || commentContext.exact || ""}>{commentTarget}</p>{/if}
      {#if diagnostic}
        <p class="panel-meta task-target" title={diagnosticTarget}>{diagnosticTarget}</p>
        {#if !contextTask}<div class="attachment-actions"><button class="btn btn-sm" onclick={() => { diagnostic = null; diagnosticRevision = ""; }}>Remove diagnostic</button></div>{/if}
      {/if}
      {#if !contextTask && taskScopes(chosen).length > 1}
        <label class="label task-scope">Scope
          <select class="select" value={scope} onchange={(event) => scope = event.currentTarget.value}>
            {#each taskScopes(chosen) as id}
              <option value={id} disabled={!scopeAvailable(id)}>{scopeLabel(id, { path: contextPath, attached: id === "selection" ? attachment : null })}</option>
            {/each}
          </select>
        </label>
      {/if}
      {#if scope === "selection" && attachment && !commentTarget}
        <div class="attachment">
          <strong>{attachment.anchored ? "Selected text" : "Quoted text"}, {selectedWords} {selectedWords === 1 ? "word" : "words"} · {attachment.path || path || "document"}</strong>
          <blockquote title={selectedText}>{selectedText}</blockquote>
          <div class="attachment-actions"><button class="btn btn-sm" onclick={removeSelection}>Remove</button><button class="btn btn-sm" disabled={!selection?.exact} onclick={replaceSelection}>Replace with current selection</button></div>
          {#if !attachment.anchored}<p class="panel-muted" role="alert">This passage has no source anchor. Explain remains available; Tighten and Rewrite need an anchored source selection.</p>{/if}
        </div>
      {/if}
      <textarea class="input task-extra" rows="1" placeholder="Additional instructions (optional)" aria-label="Additional instructions"
                value={extra} oninput={(event) => extra = event.currentTarget.value}
                onkeydown={(event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) { event.preventDefault(); void sendTask(); } }}></textarea>
      {#if staleReason || blocker || !agentReady}
        <p class="panel-meta" role="status">{staleReason || blocker || "Start an agent in the Chat tab before sending a task."}</p>
      {/if}
      <div class="prepare-actions"><button class="btn btn-sm lp-control-brand" disabled={busy || Boolean(staleReason) || Boolean(blocker) || !agentReady} onclick={() => void sendTask()}>Send to agent</button></div>
    </div>
  {:else}
    <div class="agent-context task-launcher">
      <input bind:this={searchEl} class="input task-search" type="search" role="combobox" placeholder="Search tasks..." aria-label="Search tasks"
             aria-autocomplete="list" aria-expanded={launcherRows.length > 0} aria-controls="agent-task-results"
             aria-activedescendant={launcherRows[highlight] ? `task-option-${launcherRows[highlight].id}` : undefined}
             value={taskQuery} oninput={(event) => { taskQuery = event.currentTarget.value; highlight = 0; }}
             onkeydown={searchKeydown} />
      {#if browsing}
        <div class="task-browse-head">
          <button class="task-back" aria-label="Back" onclick={stopBrowse}>← Back</button>
          <select class="select task-category" aria-label="Category" value={category}
                  onchange={(event) => { category = event.currentTarget.value; highlight = 0; }}>
            <option value="">All categories</option>
            {#each TASK_CATEGORIES as name}<option value={name}>{name}</option>{/each}
          </select>
        </div>
      {/if}
      <ul role="listbox" id="agent-task-results" class="task-results" aria-label="Tasks" bind:this={listEl} hidden={!launcherRows.length}>
        {#each launcherRows as entry, index (entry.id)}
          <!-- The search box owns the keyboard; the row is its pointer target. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li role="option" id="task-option-{entry.id}" aria-selected={index === highlight} class="task-row" class:active={index === highlight}
              title="{entry.label}: {entry.description}" onclick={() => openTask(entry)}>{entry.label}</li>
        {/each}
      </ul>
      {#if !launcherRows.length}<p class="panel-muted">No matching tasks</p>{/if}
      {#if !browsing && !taskQuery.trim()}<button class="task-browse" onclick={startBrowse}>Browse all tasks</button>{/if}
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
  /* The whole not-ready warning: one line, the desktop app notice, and the
     refusal message if there is one. */
  .connect-required { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5);
                       padding:calc(var(--spacing) * 2); background:var(--color-subtle);
                       border-left:3px solid var(--color-brand); }
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
  /* The task list is typography, spacing and a hover background, not a stack
     of controls. Rows are one line each. */
  .task-launcher, .task-prepare { gap:var(--spacing); }
  .task-results { list-style:none; margin:0; padding:0; display:flex; flex-direction:column; }
  .task-results[hidden] { display:none; }
  .task-row { padding:.375rem .5rem; border-radius:var(--radius-base, .25rem); cursor:pointer;
              white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
  .task-row:hover, .task-row.active { background:var(--color-subtle); }
  .task-search { width:100%; min-width:0; }
  .task-browse { align-self:flex-start; border:0; background:transparent; padding:0 .5rem; cursor:pointer;
                 font-size:.8em; color:var(--color-text-secondary); }
  .task-browse:hover { color:inherit; text-decoration:underline; }
  .task-browse-head { display:flex; align-items:center; justify-content:space-between; gap:var(--spacing); }
  .task-category { min-width:0; max-width:60%; }
  .task-head { display:flex; align-items:center; gap:var(--spacing); }
  .task-back { border:0; background:transparent; padding:0; cursor:pointer; color:var(--color-text-secondary); }
  .task-back:hover { color:inherit; }
  .task-title { margin:0; font-weight:600; min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .task-description { margin:0; font-size:.85em; }
  .task-target { margin:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .task-scope .select, .task-prepare .input { width:100%; min-width:0; }
  .task-extra { field-sizing:content; min-height:2rem; max-height:8rem; overflow:auto; resize:none; }
  .prepare-actions { display:flex; justify-content:flex-end; }
  .context-summary { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  .attachment { display:flex; flex-direction:column; gap:var(--spacing); padding:calc(var(--spacing) * 1.5); border-left:3px solid var(--color-brand); background:var(--color-subtle); }
  .attachment blockquote { display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:3; line-clamp:3; overflow:hidden; margin:0; white-space:pre-wrap; overflow-wrap:anywhere; }
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
</style>
