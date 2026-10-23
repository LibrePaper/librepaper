<script>
  import { onMount, tick } from "svelte";
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { getPrivate, post, SHELL_HEADERS, keyHeaders, withoutKey } from "../../lib/api.js";
  import PanelHeader from "../PanelHeader.svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import ChatTranscript from "../ChatTranscript.svelte";
  import ChatComposer from "../ChatComposer.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import { createAgentClient, documentKey } from "../../lib/agent-client.svelte.js";
  import * as local from "../../lib/desktop/client.js";
  import {
    captureAttachment, capabilityAllows, diagnosticLabel,
    normalizeCapabilities, scopeLabel, visibleResults,
    TASK_CATEGORIES, findTask, frequentTasks, inferScope, recordTaskUse, searchTasks, taskBlocker, taskForKind,
    taskHint, taskPrompt, taskScopes, taskSections,
  } from "../../lib/assistant.js";
  import { humanizeTool, permissionAction, permissionButtons } from "../../lib/agent-permission.js";

  let {
    slug, link, canShare = false, path = "", selection = null, revision = "", request = null,
    diagnostics = [],
    comments = [], changes = [], onreview, onpreview, onsettings,
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
  const connecting = $derived(starting || connection.status === "connecting");
  let problem = $state("");
  let restoreProblem = $state("");
  let attachment = $state(null);
  let task = $state(null);
  let scope = $state("selection");
  let diagnostic = $state(null);
  let diagnosticRevision = $state("");
  let draft = $state("");
  // The Tasks pane is one compact browser of the whole catalog: frequently used
  // tasks first, then sections that expand and collapse, with search above them.
  // A task opens a preparation view. The query, expanded sections and scroll
  // position survive a visit to that view.
  let taskView = $state("launcher");
  let taskQuery = $state("");
  let chosenTaskId = $state("");
  let extra = $state("");
  let listEl = $state(null);
  let searchEl = $state(null);
  let backEl = $state(null);
  let savedScroll = 0;
  let returnFocus = "";
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
  // The access levels, as a choice rather than commands: each row carries its
  // own explanation, so the panel never defines them twice. There is no
  // read-only level. The document's MCP surface admits a commenter at minimum
  // and answers a reader link with a flat 404 (server/mcp.rs), so an assistant
  // on a read link would start with no tools and could not say why. Comment is
  // the narrowest level that works, and it writes only comments. Edit is needed
  // for tracked changes.
  const roles = [
    { id: "commenter", label: "Comment", help: "Read and comment. Cannot change text" },
    { id: "editor", label: "Edit", help: "Make tracked changes and apply edits when asked", warn: true },
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
  const EXPANDED_TASKS = "librepaper.agent.expandedTasks";
  const TASK_USAGE = "librepaper.agent.taskUsage";
  function lastUsed(key) { try { return localStorage.getItem(key) || ""; } catch { return ""; } }
  function rememberLastUsed(key, value) { try { localStorage.setItem(key, value); } catch { /* private window */ } }
  /// The sections open in the last browser session. With nothing valid saved,
  /// the first section is open. An empty saved list means every section is closed.
  function storedExpanded() {
    try {
      const names = JSON.parse(lastUsed(EXPANDED_TASKS) || "null");
      return Array.isArray(names) ? names.filter((name) => TASK_CATEGORIES.includes(name)) : [TASK_CATEGORIES[0]];
    } catch { return [TASK_CATEGORIES[0]]; }
  }
  let expanded = $state(storedExpanded());
  function storedUsage() {
    try {
      const saved = JSON.parse(lastUsed(TASK_USAGE) || "{}");
      return saved && typeof saved === "object" ? saved : {};
    } catch { return {}; }
  }
  let usage = $state(storedUsage());

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
  const runnerReady = $derived(connection.connected && connection.runnerConnected);
  // No agent attached yet, but sending can start the chosen one first.
  const canStart = $derived(!runnerReady && !assistant.running && Boolean(connection.id) && Boolean(chosenInstalled?.assistant));
  const preparedTaskValid = $derived(!task || capabilityAllows(caps, task, {
    attached: attachment || null, path: contextPath, deferAccess: canStart,
  }));
  const uncertainDelivery = $derived(Object.values(connection.tasks || {}).some((item) =>
    item?.delivery === "uncertain" && item.request === draft.trim()));
  const sendable = $derived(runnerReady && preparedTaskValid && !uncertainDelivery);
  const startable = $derived(canStart && preparedTaskValid && !uncertainDelivery);
  const searching = $derived(taskQuery.trim() !== "");
  const results = $derived(searching ? searchTasks(taskQuery.trim()) : []);
  const sections = $derived(searching ? [] : taskSections());
  const frequent = $derived(searching ? [] : frequentTasks(usage));
  const chosen = $derived(findTask(chosenTaskId));
  const COMMENT_TASKS = ["address-comment"];
  const contextTask = $derived(COMMENT_TASKS.includes(chosenTaskId) || chosenTaskId === "fix-diagnostic");
  const scopeAvailable = (id) => id === "selection" ? Boolean(attachment) : id === "file" ? Boolean(contextPath) : true;
  // Rows are never disabled. A row names what it is missing, and the preparation view gives the full reason.
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
    attached: attachment || null, path: contextPath, deferAccess: canStart,
  }) : "");
  // Tracked-change tasks need the agent to hold Edit, whatever the reader holds.
  const accessGap = $derived(Boolean(chosen) && ["tighten", "rewrite", "proofread", "fix"].includes(chosen.kind) && offeredAccess() !== "editor");
  const canGrantEdit = $derived(roleOffered({ id: "editor" }));
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

  const progress = $derived(
    activeTask?.status === "working"
      ? `${runningLabel} ·${humanizeTool(activeTask.message) || 'Working'}… · ${formatElapsed(elapsedTime)}`
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

  /// A drag reports its selection as it grows, so a range that overlaps or
  /// touches the last one is the same gesture and replaces the attachment.
  function continuesSelection(previous, next) {
    const range = (value) => {
      const source = value?.source || value;
      const start = source?.position;
      if (!Number.isInteger(start) || start < 0) return null;
      return { path: source.path || "", start, end: start + (source.exact || value.exact || "").length };
    };
    const before = range(previous);
    const after = range(next);
    if (!before || !after || before.path !== after.path) return false;
    return after.start <= before.end && before.start <= after.end;
  }

  /// Mint the access link for `mode` and confirm it really grants that
  /// access. The local app is then told this link directly when the
  /// assistant starts, so no connection name or key sits in a config file,
  /// a command line or a chat transcript.
  async function prepareAccessLink(mode) {
    if (canShare) {
      const endpoint = `/api/documents/${encodeURIComponent(slug)}/share`;
      const sharing = await getPrivate(endpoint);
      let accessLink = sharing.links?.find((one) => one.role === mode && one.name === "Agent" && one.key);
      if (!accessLink) {
        const answer = await post(endpoint, { create: { kind: "link", role: mode, until: "7d", name: "Agent" } });
        accessLink = answer.created;
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
        if (!stopped) await local.renewAssistant({ link: withoutKey(target), conversation: conversationId, agentToken });
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
      await local.stopAssistant({ link: withoutKey(agentLink || link), conversation: connection.id });
      assistant = { running: false, agent: "", access: "" };
    }
    await prepareAccessLink(mode);
    const agentToken = await mintAgentGrant(agentLink || link);
    const answer = await local.startAssistant({
      link: withoutKey(agentLink || link), conversation: connection.id,
      chatToken: connection.token, agentToken, agent: chosenAgent, access: mode,
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
      const answer = await local.assistantStatus({ link: withoutKey(agentLink || link), conversation: conversationId });
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
      task = { kind: "respond", scope };
    }
    if (next.diagnostic) task = next.task || { kind: "fix", scope };
    chosenTaskId = next.comment ? "address-comment"
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
    if (taskView !== "prepare") savedScroll = listEl?.closest('[role="tabpanel"]')?.scrollTop || 0;
    taskView = "prepare";
    void tick().then(() => backEl?.focus());
  }

  function leavePrepare({ focus = true } = {}) {
    taskView = "launcher";
    const row = returnFocus;
    returnFocus = "";
    void tick().then(() => {
      const pane = listEl?.closest('[role="tabpanel"]');
      if (pane) pane.scrollTop = savedScroll;
      if (focus) (row && listEl?.querySelector(`[data-row="${CSS.escape(row)}"]`) || searchEl)?.focus({ preventScroll: true });
    });
  }

  function openTask(entry, { row = "" } = {}) {
    if (!entry) return;
    chosenTaskId = entry.id;
    returnFocus = row;
    extra = "";
    scope = inferScope(entry, { attached: attachment, path: contextPath });
    enterPrepare();
  }

  function toggleSection(name) {
    expanded = expanded.includes(name) ? expanded.filter((item) => item !== name) : [...expanded, name];
    rememberLastUsed(EXPANDED_TASKS, JSON.stringify(expanded));
  }

  const taskItems = () => [...(listEl?.querySelectorAll(".task-section, .task-row") || [])];

  function searchKeydown(event) {
    if (event.isComposing) return;
    if (event.key === "ArrowDown") {
      const item = taskItems()[0];
      if (item) { event.preventDefault(); item.focus(); }
    } else if (event.key === "Enter") {
      if (!searching || !results[0]) return;
      event.preventDefault();
      openTask(results[0]);
    } else if (event.key === "Escape" && taskQuery) {
      taskQuery = "";
      event.preventDefault();
    }
  }

  /// A slash anywhere in the launcher, outside a text field, starts a search.
  function launcherKeydown(event) {
    if (event.key !== "/" || event.target.matches?.("input, textarea, select, [contenteditable]")) return;
    event.preventDefault();
    searchEl?.focus();
  }

  function resultsKeydown(event) {
    if (event.isComposing) return;
    if (event.key === "Escape") { event.preventDefault(); searchEl?.focus(); return; }
    const items = taskItems();
    const at = items.indexOf(event.target);
    if (at < 0) return;
    const onSection = event.target.classList.contains("task-section");
    const category = event.target.dataset.category || event.target.closest("ul")?.dataset.category;
    if (event.key === "ArrowRight" && onSection && event.target.getAttribute("aria-expanded") === "false") {
      event.preventDefault();
      toggleSection(category);
      return;
    }
    if (event.key === "ArrowLeft") {
      if (onSection) {
        if (event.target.getAttribute("aria-expanded") !== "true") return;
        event.preventDefault();
        toggleSection(category);
        return;
      }
      if (!category) return;
      event.preventDefault();
      listEl.querySelector(`.task-section[data-category="${CSS.escape(category)}"]`)?.focus();
      return;
    }
    const to = event.key === "ArrowDown" ? at + 1 : event.key === "ArrowUp" ? at - 1
      : event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : null;
    if (to === null) return;
    event.preventDefault();
    if (to < 0) searchEl?.focus(); else items[Math.min(to, items.length - 1)].focus();
  }

  function prepareKeydown(event) {
    if (event.key !== "Escape" || event.target.tagName === "SELECT" || extra) return;
    event.preventDefault();
    leavePrepare();
  }

  /// Picking a task and confirming it is the one decision. A send that fails
  /// leaves the preparation view as it was so the user can retry.
  async function sendTask() {
    if (busy || !chosen || staleReason || blocker || accessGap || !agentReady) return;
    const entry = chosen;
    const fromContext = contextTask;
    task = { kind: entry.kind, scope };
    const note = extra.trim();
    const prompt = taskPrompt(entry, scope);
    if (!fromContext && entry.kind !== "explain") { diagnostic = null; diagnosticRevision = ""; }
    if (!await send(note ? `${prompt}\n\n${note}` : prompt)) return;
    if (!fromContext) {
      usage = recordTaskUse(usage, entry.id);
      rememberLastUsed(TASK_USAGE, JSON.stringify(usage));
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
        // Starting the agent verified access, so a deferred task is checked again here.
        const refused = task ? taskBlocker(caps, task, { attached: attachment || null, path: contextPath }) : "";
        if (refused) throw new Error(refused);
        await client.untilRunner(RUNNER_JOIN_MS);
      }
      await client.send(text, {
        task: task || undefined, attachment,
        thread: task?.kind === "respond" ? commentContext : null,
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
      if (assistant.running) await local.stopAssistant({ link: withoutKey(agentLink || link), conversation: connection.id });
      assistant = { running: false, agent: "", access: "" };
      await client.end(); await client.create();
      draft = ""; task = null; attachment = null; commentContext = null;
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
    const reviewable = visibleResults({ context: { results } }, changes);
    if (reviewable.changes.length) onreview?.(reviewable);
    else problem = "These changes are no longer pending.";
  }

  $effect(() => {
    if (!selection?.exact) return;
    const same = selectionKey(lastSelection) === selectionKey(selection);
    if (selectionKey(selection) === suppressedSelection) return;
    if (!attachment || (same && !attachment.revision && (selection.revision || revision)) || (!same && continuesSelection(lastSelection, selection))) {
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
  <!-- Which agent, with what access: one quiet line, since both nearly always
       stay as they were last time. Sending starts the agent; changing either
       while it runs restarts it. It sits in the header of both tabs, since the
       session belongs to the whole pane. -->
  {#snippet agentSettings()}
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
    {:else}
      <span>No supported coding agent was found on this computer.</span>
    {/if}
  </div>
  {/snippet}
  <!-- The gear at the top of each pane is the one way to the desktop app
       settings now; the Chat pane also gets New conversation beside it. -->
  {#snippet paneHeader(withNewConversation)}
    <div class="pane-header">
      {#if paired}{@render agentSettings()}{/if}
      <span class="pane-actions">
      {#if withNewConversation && connection.id}
        <IconButton icon="eraser" label="New conversation" tone="plain" size="btn-icon-sm"
                    disabled={busy} onclick={() => void newConversation()} />
      {/if}
      <IconButton icon="sliders" label="Agent settings" tone="plain" size="btn-icon-sm"
                  onclick={() => onsettings?.()} />
      </span>
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
    {#if !connection.id}<button class="btn lp-control-brand" disabled={busy || connecting} onclick={() => void act(() => client.create())}>{connecting ? "Connecting…" : "Retry connection"}</button>{/if}
    {#if connection.id && !connection.connected}
      <div role="status"><span class="panel-muted">Reconnecting…</span> <button class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => void reconnect()}>Reconnect now</button></div>
    {/if}

  {#each uncertainTasks() as item (item.id)}
    <p class="panel-meta" role="alert">Message delivery was not confirmed. <button class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => void retryTask(item.id)}>Retry delivery</button></p>
  {/each}
  <ChatTranscript messages={connection.messages}
                  emptyTitle="No messages yet"
                  emptyText={connection.runnerConnected ? "Messages go to the agent on this computer." : "Send a message to start the agent."}
                  onresult={chooseResult} after={transcriptEnd}
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
  <!-- Why sending cannot start this agent, or what its route cannot do. -->
  {#if chosenInstalled?.assistant_blocked || chosenInstalled?.assistant_note}
    <!-- An older desktop app sends no assistant_docs, so the link falls back to the agents page. -->
    <p class="panel-meta" data-agent-note={chosenInstalled.id}>{chosenInstalled.assistant_blocked}{chosenInstalled.assistant_note}{#if chosenInstalled.assistant_blocked} <a href={chosenInstalled.assistant_docs || "https://librepaper.org/agents.html"} target="_blank" rel="noreferrer">Setup instructions</a>{/if}</p>
  {/if}
  {#if assistant.agent !== chosenAgent && chosenInstalled?.assistant_fetches}
    <p class="panel-meta" data-adapter-fetch>Starting it downloads its adapter.{#if chosenInstalled.assistant_install}{" "}To install it for good, run <code>{chosenInstalled.assistant_install}</code>.{/if}</p>
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
          {#if !attachment.anchored}<p class="panel-muted" role="alert">This passage was selected in the preview. Select it in the source for Tighten and Rewrite.</p>{/if}
        </div>
      {/if}
      <textarea class="input task-extra" rows="1" placeholder="Additional instructions (optional)" aria-label="Additional instructions"
                value={extra} oninput={(event) => extra = event.currentTarget.value}
                onkeydown={(event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) { event.preventDefault(); void sendTask(); } }}></textarea>
      {#if staleReason || accessGap || blocker || !agentReady}
        <p class="panel-meta" role="status">{staleReason || (accessGap ? (canGrantEdit ? "This task makes tracked changes. Set the agent's access to Edit above." : "This task makes tracked changes. You need edit access to run it.") : blocker || "Start an agent in the Chat tab before sending a task.")}</p>
      {/if}
      <div class="prepare-actions"><button class="btn btn-sm lp-control-brand" disabled={busy || Boolean(staleReason) || Boolean(blocker) || accessGap || !agentReady} onclick={() => void sendTask()}>Send to agent</button></div>
    </div>
  {:else}
    {#snippet taskRow(entry, group)}
      {@const hint = taskHint(entry, { attached: attachment, path: contextPath })}
      {@const describedBy = `task-desc-${group.replace(/\W+/g, "-")}-${entry.id}`}
      <li>
        <button class="task-row" class:active={searching && entry.id === results[0]?.id} class:unavailable={Boolean(hint)}
                data-task-id={entry.id} data-row={`${group}:${entry.id}`}
                title={hint ? `${entry.description} ${hint.reason}` : entry.description}
                aria-describedby={describedBy}
                onclick={(event) => openTask(entry, { row: event.currentTarget.dataset.row })}>
          <span class="task-line">
            <span class="task-name">{entry.label}</span>
            {#if hint}<span class="task-hint">{hint.short}</span>{/if}
          </span>
          {#if searching}
            <span class="task-summary">{entry.description}</span>
            <span class="task-meta">{entry.categories.join(" / ")}</span>
          {:else}
            <span class="task-summary task-summary-focus">{entry.description}</span>
          {/if}
        </button>
        <span class="visually-hidden" id={describedBy}>{hint ? `${entry.description} ${hint.reason}` : entry.description}</span>
      </li>
    {/snippet}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="agent-context task-launcher" onkeydown={launcherKeydown}>
      <div class="task-browser-head">
        <input bind:this={searchEl} class="input task-search" type="search" placeholder="Search tasks..." aria-label="Search tasks" aria-controls="agent-task-results"
               value={taskQuery} oninput={(event) => taskQuery = event.currentTarget.value}
               onkeydown={searchKeydown} />
      </div>
      <!-- The search box and the rows own the keyboard; this only routes it. -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div id="agent-task-results" class="task-results" bind:this={listEl} onkeydown={resultsKeydown}>
        {#if searching}
          {#if results.length}<ul class="task-list" aria-label="Matching tasks">{#each results as entry (entry.id)}{@render taskRow(entry, "search")}{/each}</ul>
          {:else}<p class="panel-muted" role="status">No tasks match "{taskQuery.trim()}". Try another word, such as grammar or citations.</p>{/if}
        {:else}
          {#if frequent.length}
            <h4 class="task-group-label" id="task-frequent-label">Frequently used</h4>
            <ul class="task-list" aria-labelledby="task-frequent-label">
              {#each frequent as entry (entry.id)}{@render taskRow(entry, "frequent")}{/each}
            </ul>
          {/if}
          {#each sections as section, index (section.category)}
            {@const open = expanded.includes(section.category)}
            <button class="task-section" data-category={section.category} aria-expanded={open}
                    aria-controls={`task-section-${index}`} onclick={() => toggleSection(section.category)}>
              <span class="task-chevron" aria-hidden="true">▸</span><span class="task-section-name">{section.category}</span><span class="task-count">{section.entries.length}</span>
            </button>
            <ul class="task-list" id={`task-section-${index}`} data-category={section.category}>
              {#if open}{#each section.entries as entry (entry.id)}{@render taskRow(entry, section.category)}{/each}{/if}
            </ul>
          {/each}
        {/if}
      </div>
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
  /* The agent and access line sits at the top left of both panes, and the
     gear (and, in Chat, New conversation) at the top right. */
  .pane-header { display:flex; align-items:center; justify-content:space-between; gap:calc(var(--spacing) * .5); }
  .pane-actions { display:flex; gap:calc(var(--spacing) * .5); margin-left:auto; }
  .chat-history { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 2); overflow-y:auto; }
  .chat-history > :global(*) { flex-shrink:0; }
  .agent-panel :global(.agent-tab-content .chat-form) { flex-shrink:0; }
  .agent-context { display:flex; flex-direction:column; gap:calc(var(--spacing) * 2); }
  .agent-settings { display:flex; align-items:center; gap:calc(var(--spacing) * .5); min-width:0; font-size:.8em; color:var(--color-text-secondary); }
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
  /* The task browser is typography, spacing and a hover background, not a
     stack of controls. The head stays put while the pane scrolls under it. */
  .task-launcher, .task-prepare { gap:var(--spacing); }
  .task-browser-head { position:sticky; z-index:1; display:flex; flex-direction:column; gap:var(--spacing);
                       top:calc(var(--panel-padding) * -1); margin-top:calc(var(--spacing) * -2); padding-top:var(--panel-padding);
                       padding-bottom:var(--spacing); background:var(--color-sidebar); }
  .task-search { width:100%; min-width:0; }
  .task-results { display:flex; flex-direction:column; }
  .task-list { list-style:none; margin:0; padding:0; display:flex; flex-direction:column; }
  /* Headings sit at the rows' size and stand out by weight, so a heading is
     never smaller than the tasks under it. */
  .task-group-label { margin:0; padding:.25rem .5rem; font-size:1em; font-weight:600; }
  .task-section { display:flex; align-items:center; gap:var(--spacing); width:100%; margin-top:var(--spacing); padding:.25rem .5rem;
                  border:0; background:transparent; cursor:pointer; text-align:left; font:inherit; font-weight:600;
                  color:inherit; }
  .task-section:first-child { margin-top:0; }
  /* The chevron turns with the state and does not animate. */
  .task-chevron { flex:none; display:inline-block; width:.75em; }
  .task-section[aria-expanded="true"] .task-chevron { transform:rotate(90deg); }
  .task-count { margin-left:auto; font-size:.8em; font-weight:400; color:var(--color-text-secondary); }
  .task-row { display:flex; flex-direction:column; justify-content:center; gap:.125rem; width:100%; min-height:2.25rem;
              padding:.25rem .5rem; border:0; background:transparent; border-radius:var(--radius-base, .25rem);
              cursor:pointer; text-align:left; font:inherit; color:inherit; }
  .task-row:hover, .task-row.active { background:var(--color-subtle); }
  @media (pointer: coarse) { .task-row, .task-section { min-height:2.75rem; } }
  .task-section:focus-visible, .task-row:focus-visible { outline:2px solid var(--color-focus); outline-offset:-2px; }
  .task-line { display:flex; gap:var(--spacing); min-width:0; }
  .task-name { min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; font-weight:400; }
  .task-row.unavailable .task-name { color:var(--color-text-secondary); }
  .task-hint { flex:none; margin-left:auto; min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis;
               font-size:.75em; color:var(--color-text-secondary); }
  .task-meta { font-size:.75em; color:var(--color-text-secondary); }
  .task-summary { display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:2; line-clamp:2; overflow:hidden;
                  font-size:.8em; color:var(--color-text-secondary); }
  /* Keyboard users see the description of the focused row; pointer users get it as a tooltip. */
  .task-summary-focus { display:none; }
  .task-row:focus-visible .task-summary-focus { display:-webkit-box; }
  .visually-hidden { position:absolute; width:1px; height:1px; overflow:hidden; clip:rect(0 0 0 0); white-space:nowrap; }
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
