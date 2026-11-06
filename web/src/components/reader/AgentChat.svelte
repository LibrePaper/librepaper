<script>
  import { onMount, tick } from "svelte";
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import { getPrivate, post, SHELL_HEADERS, keyHeaders, withoutKey } from "../../lib/api.js";
  import IconButton from "../IconButton.svelte";
  import Icon from "../Icon.svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import ChatTranscript from "../ChatTranscript.svelte";
  import ChatComposer from "../ChatComposer.svelte";
  import { createAgentClient, documentKey } from "../../lib/agent-client.svelte.js";
  import * as local from "../../lib/desktop/client.js";
  import {
    captureAttachment, capabilityAllows, diagnosticLabel,
    normalizeCapabilities, scopeLabel, visibleResults,
    findTask, inferScope, taskBlocker, taskForKind, taskPrompt, taskScopes,
  } from "../../lib/assistant.js";
  import { humanizeTool, permissionAction, permissionButtons } from "../../lib/agent-permission.js";
  import { m } from "../../lib/paraglide/messages.js";

  let {
    slug, link, canShare = false, path = "", selection = null, revision = "", request = null,
    diagnostics = [],
    comments = [], changes = [], onreview, onpreview, onsettings,
    userName = m.common_you(),
    // One instance per session: the wrapper mounts each, shows only the active one,
    // and renders the session switcher and its new and delete buttons in this
    // pane's header.
    session = "", active = true, switcher = undefined, sessionActions = undefined,
    onstate = undefined,
  } = $props();
  // Several panels can be mounted at once, one per session, so element ids
  // carry the session. The first session keeps the bare ids.
  const idSuffix = $derived(session ? `-${session}` : "");
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
  // A task opens its preparation view. Back and Escape return to the conversation.
  let taskView = $state("chat");
  let chosenTaskId = $state("");
  let extra = $state("");
  let backEl = $state(null);
  let agentLink = $state("");
  // What this computer has, as the local app reports it. The browser cannot
  // read a PATH, so without the desktop app there is nothing to offer and the
  // panel says so rather than guessing.
  let installed = $state([]);
  let restoredConversation = "";
  let chosenAgent = $state("");
  let assistant = $state({ running: false, agent: "", access: "" });
  const chosenInstalled = $derived(installed.find((entry) => entry.id === chosenAgent) || null);
  // The access levels, as a choice rather than commands: each row carries its
  // own explanation, so the panel never defines them twice. There is no
  // read-only level. The document's MCP surface admits a commenter at minimum
  // and answers a reader link with a flat 404 (server/mcp.rs), so an assistant
  // on a read link would start with no tools and could not say why. Comment is
  // the narrowest level that works, and it writes only comments. Edit is needed
  // for tracked changes.
  const roles = [
    { id: "commenter", label: m.common_comment(), help: m.agent_role_comment_help() },
    { id: "editor", label: m.common_edit(), help: m.agent_role_edit_help(), warn: true },
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
      throw new Error(body.error || m.agent_sign_in_again());
    }
    return body.token;
  }
  let access = $state(roles.some((entry) => entry.id === lastUsed(LAST_ACCESS)) ? lastUsed(LAST_ACCESS) : "commenter");
  const runningLabel = $derived(installed.find((entry) => entry.id === assistant.agent)?.label || chosenInstalled?.label || m.agent_the_agent());
  let pendingRequest = $state(null);
  let commentContext = $state(null);
  let lastRequestId = "";
  let lastSelection = $state(null);
  let suppressedSelection = $state("");
  let capabilityGeneration = 0;
  let verifiedCapabilities = $state(null);
  let elapsedTime = $state(0);
  let panelEl = $state(null);
  const selectedText = $derived(attachment?.selection?.exact || "");
  const caps = $derived(normalizeCapabilities(verifiedCapabilities));
  const contextPath = $derived(diagnostic?.file || diagnostic?.path || path);
  // What the composer's context selector shows. A passage wins over the file,
  // and the file wins over the whole document.
  const chatScope = $derived(attachment ? "selection" : scope === "document" || !contextPath ? "document" : "file");
  const contextLabel = $derived(chatScope === "selection"
    ? (attachment?.path ? m.agent_context_selection_path({ path: attachment.path }) : m.agent_context_selection())
    : chatScope === "file" ? contextPath : m.common_whole_document());
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
  const chosen = $derived(findTask(chosenTaskId));
  const COMMENT_TASKS = ["address-comment"];
  const contextTask = $derived(COMMENT_TASKS.includes(chosenTaskId) || chosenTaskId === "fix-diagnostic");
  const scopeAvailable = (id) => id === "selection" ? Boolean(attachment) : id === "file" ? Boolean(contextPath) : true;
  // The preparation view gives the full reason a task cannot run.
  const staleReason = $derived.by(() => {
    if (!chosen) return "";
    if (COMMENT_TASKS.includes(chosenTaskId) && commentContext) {
      const live = comments.find((item) => String(item.id) === String(commentContext.id));
      if (!live || live.resolved) return m.agent_comment_gone();
    }
    if (diagnostic) {
      const file = diagnostic.file || diagnostic.path || "";
      const present = diagnostics.some((item) => (item.file || item.path || "") === file
        && (item.line || 0) === (diagnostic.line || 0) && item.message === diagnostic.message);
      if (!present) return m.agent_diagnostic_gone();
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
    ? m.agent_comment_target({ text: clip(commentContext.body || commentContext.exact || "", 80) }) : "");
  const diagnosticTarget = $derived(diagnostic
    ? (diagnostic.message
      ? `${contextPath}${diagnostic.line ? `:${diagnostic.line}` : ""} ${diagnostic.message}`.trim()
      : diagnosticLabel(diagnostic, path))
    : "");
  const selectedWords = $derived(selectedText.trim().split(/\s+/).filter(Boolean).length);
  function clip(text, max) { return text.length > max ? `${text.slice(0, max)}...` : text; }
  const inputTask = $derived(Object.values(connection.tasks || {}).find((item) => item?.status === "needs_input" && item.input) || null);
  // Everything that is currently in flight. "Active" (as opposed to queued) is
  // what Stop can actually interrupt.
  const activeTasks = $derived(Object.values(connection.tasks || {}).filter((item) =>
    ["queued", "working", "needs_input"].includes(item.status)));
  const activeTask = $derived(activeTasks.find((item) => ["working", "needs_input"].includes(item.status)) || null);
  const queuedTasks = $derived(Object.values(connection.tasks || {}).filter((item) => item?.status === "queued"));
  // The first queued request the conversation shows, so View has a message to scroll to.
  const viewableQueued = $derived(queuedTasks.find((item) =>
    connection.messages.some((message) => message.role === "user" && message.id === item.id)) || null);
  const queueCount = $derived(queuedTasks.length);

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

  const activityText = $derived.by(() => {
    if (activeTask?.status === "working") return `${humanizeTool(activeTask.message) || m.agent_is_working({ agent: runningLabel })}… · ${formatElapsed(elapsedTime)}`;
    if (activeTask?.status === "needs_input") return m.agent_waiting_permission();
    return "";
  });
  const queueText = $derived(queueCount ? m.agent_queue({ count: queueCount }) : "");

  function viewQueued() {
    const id = viewableQueued?.id;
    if (!id) return;
    const message = panelEl?.querySelector(`.chat-message[data-id="${CSS.escape(id)}"]`);
    message?.scrollIntoView({ block: "nearest" });
    message?.querySelector(".queued-note button")?.focus({ preventScroll: true });
  }

  /// The composer's context choice. Picking a passage attaches the current
  /// selection when none is attached; picking a file or the whole document
  /// drops the passage.
  function chooseContext(value) {
    if (value === "selection") {
      if (!attachment) replaceSelection();
      scope = "selection";
    } else {
      if (attachment) removeSelection();
      scope = value;
    }
    if (task) task = { ...task, scope };
  }

  function validDocumentLink() {
    try {
      const chosen = new URL(withoutKey(agentLink || link), location.href);
      const page = new URL(withoutKey(link), location.href);
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
      if (!accessLink?.url) throw new Error(m.agent_link_create_failed());
      agentLink = new URL(accessLink.url, location.origin).href;
    }
    if (!validDocumentLink()) throw new Error(m.agent_link_wrong_document());
    await checkCapabilities(agentLink, { strict: true });
    if (currentRole !== mode) {
      const role = mode === "editor" ? m.agent_role_word_edit() : mode === "commenter" ? m.agent_role_word_comment() : m.agent_role_word_read();
      throw new Error(m.agent_link_no_grant({ role }));
    }
  }

  async function refreshAgents() {
    const answer = await local.agents();
    installed = Array.isArray(answer?.agents) ? answer.agents : [];
    if (!chosenAgent) chosenAgent = installed.find((entry) => entry.assistant && entry.id === lastUsed(LAST_AGENT))?.id
      || installed.find((entry) => entry.assistant)?.id || installed[0]?.id || "";
  }
  $effect(() => { void act(refreshAgents); });
  $effect(() => {
    if (!connectionId) {
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
        !stopped && connectionId === conversationId);
      if (restored || stopped || connectionId !== conversationId) return;
      timer = setTimeout(() => void restore(), retryDelay);
      retryDelay = Math.min(retryDelay * 2, 30_000);
    };
    void restore();
    return () => { stopped = true; clearTimeout(timer); };
  });
  $effect(() => {
    if (!assistant.running || !connectionId) return;
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
    if (!answer?.running) throw new Error(answer?.error || m.agent_not_started());
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
    // A request naming only a catalog task opens that task's preparation view.
    const byId = !next.comment && !next.diagnostic && !next.task && next.taskId ? findTask(next.taskId) : null;
    chosenTaskId = next.comment ? "address-comment"
      : next.diagnostic ? (next.task?.kind === "explain" ? taskForKind("explain")?.id || "" : "fix-diagnostic")
      : next.task ? taskForKind(next.task.kind)?.id || "" : "";
    lastRequestId = next.id || lastRequestId;
    if (byId) openTask(byId);
    else if (chosenTaskId) enterPrepare();
    else taskView = "chat";
  }

  function requestArrived(next) {
    if (!next?.id || next.id === lastRequestId) return;
    lastRequestId = next.id;
    if (taskView === "prepare" && extra.trim()) pendingRequest = next;
    else applyRequest(next);
  }

  function enterPrepare() {
    taskView = "prepare";
    void tick().then(() => backEl?.focus());
  }

  /// Back and Escape both return to the conversation, with focus in the composer.
  function leavePrepare() {
    taskView = "chat";
    void tick().then(() => panelEl?.querySelector(".chat-form textarea")?.focus());
  }

  function openTask(entry) {
    if (!entry) return;
    chosenTaskId = entry.id;
    extra = "";
    scope = inferScope(entry, { attached: attachment, path: contextPath });
    enterPrepare();
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
    extra = "";
    leavePrepare();
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
      try {
        await client.send(text, {
          task: task || undefined, attachment,
          thread: task?.kind === "respond" ? commentContext : null,
          path: (task ? task.scope === "file" : chatScope !== "document") ? contextPath : "",
          revision: diagnosticRevision || attachment?.revision || (task?.scope === "selection" ? revision : ""),
          diagnostic, diagnostics,
        });
        sent = true;
      } catch (error) {
        // The message is already in the transcript, so the composer must not keep it.
        if (error?.recorded) sent = true;
        throw error;
      }
    });
    return sent;
  }

  /// Stop this conversation's agent and end its channel, before the session is deleted.
  export async function close() {
    if (assistant.running) await local.stopAssistant({ link: withoutKey(agentLink || link), conversation: connection.id }).catch(() => {});
    assistant = { running: false, agent: "", access: "" };
    await client?.end();
  }

  async function reconnect() { await act(() => client.reconnect()); }

  function uncertainTasks() { return Object.values(connection.tasks || {}).filter((item) => item?.delivery === "uncertain"); }
  async function retryTask(id) {
    await act(async () => {
      if (!await client.retry(id)) throw new Error(m.agent_retry_gone());
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
    else problem = m.agent_changes_gone();
  }

  // Only the active session takes the editor selection. Reading `active` first
  // makes this effect run again when the session is switched to.
  $effect(() => {
    if (!active || !selection?.exact) return;
    const same = selectionKey(lastSelection) === selectionKey(selection);
    if (selectionKey(selection) === suppressedSelection) return;
    if (!attachment || (same && !attachment.revision && (selection.revision || revision)) || (!same && continuesSelection(lastSelection, selection))) {
      attach(selection, selection.revision || revision);
    }
  });
  $effect(() => requestArrived(request));
  $effect(() => onstate?.({
    agent: assistant.running || connection.assistantAgent ? runningLabel : "",
    working: activeTask?.status === "working",
    needsInput: Boolean(inputTask),
  }));
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
    client = createAgentClient({ origin: location.origin, slug, link: agentLink || link, onpreview, session });
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

  <!-- Which agent, with what access: one quiet line, since both nearly always
       stay as they were last time. Sending starts the agent; changing either
       while it runs restarts it. It sits in the pane header, since the
       session belongs to the whole pane. -->
  {#snippet agentSettings()}
  <div class="agent-settings">
    {#if installed.length}
      <select class="agent-setting" aria-label={m.common_agent()} disabled={busy} value={chosenAgent}
              onchange={(event) => void choose(() => chosenAgent = event.currentTarget.value)}>
        {#each installed as entry (entry.id)}<option value={entry.id}>{entry.label}</option>{/each}
      </select>
      <span aria-hidden="true">·</span>
      <select class="agent-setting" aria-label={m.agent_access_label()} disabled={busy || !connection.id} value={access}
              onchange={(event) => void choose(() => access = event.currentTarget.value)}>
        {#each roles as entry}<option value={entry.id} disabled={!roleOffered(entry)} title={entry.help}>{entry.label}</option>{/each}
      </select>
    {:else}
      <span>{m.agent_none_found()}</span>
    {/if}
  </div>
  {/snippet}
  <!-- The header has two lines. The first is the session switcher, supplied by
       the wrapper, which takes the full width so its label is never cut short.
       The second holds the agent and access settings on the left, and on the
       right the session actions (new, delete) followed by the gear, the one way
       to the desktop app settings. Narrow panels wrap within the same order. -->
  {#snippet paneHeader()}
    <div class="pane-header">
      {#if switcher}<div class="pane-switcher">{@render switcher()}</div>{/if}
      {@render agentSettings()}
      <span class="pane-actions">
      {#if sessionActions}{@render sessionActions()}{/if}
      <IconButton icon="sliders" label={m.agent_settings_label()} tone="plain" size="btn-icon-sm"
                  onclick={() => onsettings?.()} />
      </span>
    </div>
  {/snippet}

  <!-- The permission card sits inside the transcript, through ChatTranscript's
       `after` snippet, so it reads as part of the conversation rather than a
       dashboard bolted underneath it. -->
  {#snippet permissionCard()}
    {#if inputTask}
      <div class="permission-card" role="group" aria-label={m.agent_input_request_label()}>
        <span class="permission-label">{m.agent_permission_required()}</span>
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
            <summary>{m.common_details()}</summary>
            <div class="permission-details">
              {#if details.command}<p><strong>{m.common_command()}</strong><code>{details.command.slice(0, 2048)}{#if details.command.length > 2048}…{/if}</code></p>{/if}
              {#if details.files?.length}<p><strong>{m.common_files()}</strong><span>{details.files.slice(0, 8).map((path) => String(path).slice(0, 512)).join(", ")}{#if details.files.length > 8} {m.agent_files_more({ count: details.files.length - 8 })}{/if}</span></p>{/if}
              {#if details.diff}<details><summary>{m.common_review_changes()}</summary><pre>{details.diff.slice(0, 2048)}{#if details.diff.length > 2048}…{/if}</pre></details>{/if}
            </div>
          </details>
        {/if}
      </div>
    {/if}
  {/snippet}

  <!-- The permission card is the last thing in the conversation. -->
  {#snippet transcriptEnd()}
    {@render permissionCard()}
  {/snippet}

  <!-- A queued request says so under its own message, with Cancel while it waits. -->
  {#snippet queuedNote(message)}
    {#if message.role === "user" && connection.tasks?.[message.id]?.status === "queued"}
      {@const item = connection.tasks[message.id]}
      <p class="queued-note" data-task-id={message.id}>{m.agent_queued()}{#if item.cancelRequested} · {m.agent_cancelling()}{:else} · <button type="button" class="agent-text-action" disabled={busy || !runnerReady} onclick={() => void act(() => client.cancel(message.id))}>{m.common_cancel()}</button>{/if}</p>
    {/if}
  {/snippet}

  <!-- The context choice sits inside the composer, as a quiet trigger. -->
  {#snippet chatContext()}
    <Menu onSelect={({ value }) => chooseContext(value)}>
      <Menu.Trigger class="context-trigger" aria-label={m.agent_context_label({ label: contextLabel })} title={contextLabel}>
        <span class="context-label">{contextLabel}</span><Icon name="chevron-down" size="0.875rem" />
      </Menu.Trigger>
      <ExplorerMenu>
        <Menu.Item value="selection" class="menuitem" disabled={!attachment && !selection?.exact}>
          <span class="menuitem-check" aria-hidden="true">{chatScope === "selection" ? "✓" : ""}</span>
          <span class="menuitem-label">{m.common_selected_passage()}</span>
        </Menu.Item>
        <Menu.Item value="file" class="menuitem" disabled={!contextPath}>
          <span class="menuitem-check" aria-hidden="true">{chatScope === "file" ? "✓" : ""}</span>
          <span class="menuitem-label">{contextPath ? m.common_current_file_path({ path: contextPath }) : m.common_current_file()}</span>
        </Menu.Item>
        <Menu.Item value="document" class="menuitem">
          <span class="menuitem-check" aria-hidden="true">{chatScope === "document" ? "✓" : ""}</span>
          <span class="menuitem-label">{m.common_whole_document()}</span>
        </Menu.Item>
      </ExplorerMenu>
    </Menu>
  {/snippet}

<div class="agent-tab-content agent-chat-pane" bind:this={panelEl}>
  {@render paneHeader()}
  {#if pendingRequest}
    <div class="request-warning" role="alert">
      <span>{m.agent_request_arrived()}</span>
      <div class="setup-actions"><button class="btn btn-sm lp-control-brand" onclick={() => applyRequest(pendingRequest)}>{m.common_replace()}</button><button class="btn btn-sm" onclick={() => pendingRequest = null}>{m.agent_keep_current()}</button></div>
    </div>
  {/if}
  {#if taskView === "prepare" && chosen}
    <!-- Escape is a convenience on top of the Back button. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="agent-context task-prepare" onkeydown={prepareKeydown}>
      <div class="task-head">
        <button class="task-back" aria-label={m.agent_back_tasks()} bind:this={backEl} onclick={() => leavePrepare()}><Icon name="arrow-left" size={18} /></button>
        <h3 class="task-title">{chosen.label}</h3>
      </div>
      <p class="panel-muted task-description">{chosen.description}</p>
      {#if commentTarget}<p class="panel-meta task-target" title={commentContext.body || commentContext.exact || ""}>{commentTarget}</p>{/if}
      {#if diagnostic}
        <p class="panel-meta task-target" title={diagnosticTarget}>{diagnosticTarget}</p>
        {#if !contextTask}<div class="attachment-actions"><button class="btn btn-sm" onclick={() => { diagnostic = null; diagnosticRevision = ""; }}>{m.agent_remove_diagnostic()}</button></div>{/if}
      {/if}
      {#if !contextTask && taskScopes(chosen).length > 1}
        <label class="label task-scope">{m.agent_scope()}
          <select class="select" value={scope} onchange={(event) => scope = event.currentTarget.value}>
            {#each taskScopes(chosen) as id}
              <option value={id} disabled={!scopeAvailable(id)}>{scopeLabel(id, { path: contextPath, attached: id === "selection" ? attachment : null })}</option>
            {/each}
          </select>
        </label>
      {/if}
      {#if scope === "selection" && attachment && !commentTarget}
        <div class="attachment">
          <strong>{m.agent_attachment_summary({ kind: attachment.anchored ? m.agent_selected_text() : m.agent_quoted_text(), count: selectedWords, path: attachment.path || path || m.agent_document_word() })}</strong>
          <blockquote title={selectedText}>{selectedText}</blockquote>
          <div class="attachment-actions"><button class="btn btn-sm" onclick={removeSelection}>{m.common_remove()}</button><button class="btn btn-sm" disabled={!selection?.exact} onclick={replaceSelection}>{m.agent_replace_selection()}</button></div>
          {#if !attachment.anchored}<p class="panel-muted" role="alert">{m.agent_preview_passage_note()}</p>{/if}
        </div>
      {/if}
      <textarea class="input task-extra" rows="1" placeholder={m.agent_extra_placeholder()} aria-label={m.agent_extra_label()}
                value={extra} oninput={(event) => extra = event.currentTarget.value}
                onkeydown={(event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) { event.preventDefault(); void sendTask(); } }}></textarea>
      {#if staleReason || accessGap || blocker || !agentReady}
        <p class="panel-meta" role="status">{staleReason || (accessGap ? (canGrantEdit ? m.agent_task_tracked_grant() : m.agent_task_tracked_no_access()) : blocker || m.agent_start_first())}</p>
      {/if}
      <div class="prepare-actions"><button class="btn btn-sm lp-control-brand" disabled={busy || Boolean(staleReason) || Boolean(blocker) || accessGap || !agentReady} onclick={() => void sendTask()}>{m.agent_send_to_agent()}</button></div>
    </div>
  {:else}
  <div class="chat-history">
  {#if !connection.id}<button class="btn lp-control-brand" disabled={busy || connecting} onclick={() => void act(() => client.create())}>{connecting ? m.common_connecting() : m.agent_retry_connection()}</button>{/if}
  {#if connection.id && !connection.connected}
    <div role="status"><span class="panel-muted">{m.common_reconnecting()}</span> <button class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => void reconnect()}>{m.agent_reconnect_now()}</button></div>
  {/if}

  {#each uncertainTasks() as item (item.id)}
  <p class="panel-meta" role="alert">{m.agent_delivery_unconfirmed()} <button class="btn btn-sm lp-control-tonal-brand" disabled={busy} onclick={() => void retryTask(item.id)}>{m.agent_retry_delivery()}</button></p>
  {/each}
  <ChatTranscript messages={connection.messages}
                emptyTitle={m.common_no_messages_yet()}
                emptyText={connection.runnerConnected ? m.agent_empty_connected() : m.agent_empty_send()}
                onresult={chooseResult} after={transcriptEnd} messageEnd={queuedNote}
                roleLabel={(message) => message.role === "user" ? userName : (message.creator || message.role)} />

  <!-- Missing access or context is worth explaining; a runner that simply
     is not there yet starts when the message is sent, so that case gets no
     line here. -->
  {#if !sendable && (uncertainDelivery || connection.runnerConnected)}
  <p class="panel-meta">{uncertainDelivery ? m.agent_not_confirmed() : m.agent_task_needs_context()}</p>
  {/if}
  </div>
  <!-- Why sending cannot start this agent, or what its route cannot do. -->
  {#if chosenInstalled?.assistant_blocked || chosenInstalled?.assistant_note}
  <!-- An older desktop app sends no assistant_docs, so the link falls back to the agents page. -->
  <p class="panel-meta" data-agent-note={chosenInstalled.id}>{chosenInstalled.assistant_blocked}{chosenInstalled.assistant_note}{#if chosenInstalled.assistant_blocked} <a href={chosenInstalled.assistant_docs || "https://librepaper.org/agents.html"} target="_blank" rel="noreferrer">{m.common_setup_instructions()}</a>{/if}</p>
  {/if}
  {#if assistant.agent !== chosenAgent && chosenInstalled?.assistant_fetches}
  <p class="panel-meta" data-adapter-fetch>{m.agent_adapter_download()}{#if chosenInstalled.assistant_install}{" "}{m.agent_adapter_install()} <code>{chosenInstalled.assistant_install}</code>.{/if}</p>
  {/if}
  <!-- The first row says what the running task is doing, with Stop.
     The second appears only while requests wait, with View and Cancel. -->
  {#if activeTask || queueCount > 0}
  <div class="agent-status-rows">
    {#if activeTask}
      <div class="agent-status">
        <p class="agent-progress" role="status">{activityText}</p>
        <span class="agent-status-actions">
          {#if assistant.running}
            {#if activeTask.cancelRequested}
              <span class="agent-status-note">{m.agent_stopping()}</span>
            {:else}
              <button type="button" class="agent-text-action" disabled={busy} onclick={() => void act(() => client.cancel(activeTask.id))}>{m.common_stop()}</button>
            {/if}
          {/if}
        </span>
      </div>
    {/if}
    {#if queueCount > 0}
      <div class="agent-status agent-queue">
        <p class="agent-progress agent-queue-text" role={activeTask ? undefined : "status"}>{queueText}</p>
        <span class="agent-status-actions">
          {#if viewableQueued}
            <button type="button" class="agent-text-action" onclick={viewQueued}>{m.agent_view()}</button>
          {/if}
          {#if queueCount === 1}
            {#if queuedTasks[0].cancelRequested}
              <span class="agent-status-note">{m.agent_cancelling()}</span>
            {:else}
              <button type="button" class="agent-text-action" disabled={busy || !runnerReady} onclick={() => void act(() => client.cancel(queuedTasks[0].id))}>{m.common_cancel()}</button>
            {/if}
          {/if}
        </span>
      </div>
    {/if}
  </div>
  {/if}
  <ChatComposer placeholder={m.agent_ask({ agent: chosenInstalled?.label || m.agent_your_agent() })} canSend={!busy && (sendable || startable)} draft={draft} ondraft={(value) => draft = value} onsend={send}
              context={chatContext} />
  {/if}
  {#if problem || restoreProblem || connection.error}<p class="panel-muted" role="alert">{problem || restoreProblem || connection.error}</p>{/if}
</div>

<style>
  /* The pane's padding is the shell's .panel-tabbed. This pane is a column that
     fills its parent: the transcript scrolls under a composer that stays put, and
     the task preparation view scrolls on its own. */
  .agent-chat-pane { display:flex; flex-direction:column; flex:1 1 0; min-height:0; gap:calc(var(--spacing) * 2); overflow:hidden; }
  .agent-chat-pane > :global(*) { flex-shrink:0; }
  /* The session switcher takes the first line of both panes. The agent and
     access line sits at the left of the second, and the actions (new, delete,
     gear) at the right. When the panel is too narrow, the line wraps and the
     actions stay at the end. */
  .pane-header { display:flex; align-items:center; flex-wrap:wrap; justify-content:space-between; gap:calc(var(--spacing) * .5); }
  .pane-switcher { flex:1 0 100%; min-width:0; }
  .pane-actions { display:flex; gap:calc(var(--spacing) * .5); margin-left:auto; }
  .chat-history { display:flex; flex:1 1 0; min-height:0; flex-direction:column; gap:calc(var(--spacing) * 2); overflow-y:auto; }
  .chat-history > :global(*) { flex-shrink:0; }
  .agent-chat-pane :global(.chat-form) { flex-shrink:0; }
  .agent-context { display:flex; flex-direction:column; gap:calc(var(--spacing) * 2); }
  .agent-settings { display:flex; align-items:center; gap:calc(var(--spacing) * .5); min-width:0; font-size:.8em; color:var(--color-text-secondary); }
  .agent-setting { border:0; background:transparent; color:inherit; font:inherit; padding:0; cursor:pointer; max-width:12rem; }
  /* Agent and access are two settings, each with one value in force, so each
     is a select. A row of buttons had to signal the chosen one and the signal
     was easy to miss; a select shows its value as its content. */
  .setup-actions, .attachment-actions { display:flex; align-items:center; flex-wrap:wrap; gap:var(--spacing); }
  /* Each status row is one line with its text actions at the right: the running
     task, then the queue. A row never wraps, so the composer does not move as it changes. */
  .agent-status-rows { display:flex; flex-direction:column; gap:calc(var(--spacing) * .5); flex-shrink:0; }
  .agent-status { display:flex; align-items:center; gap:var(--spacing); min-width:0; flex-shrink:0; }
  .agent-progress { flex:1 1 auto; min-width:0; margin:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; color:var(--panel-muted); font-size:var(--panel-meta-size); line-height:var(--panel-line-height); }
  .agent-status-actions { display:flex; flex:none; align-items:center; gap:calc(var(--spacing) * .5); }
  .agent-status-note { color:var(--panel-muted); font-size:var(--panel-meta-size); white-space:nowrap; }
  .agent-chat-pane :global(.agent-text-action) { border:0; background:transparent; padding:0 var(--spacing); border-radius:var(--radius-base); font:inherit; font-size:var(--panel-meta-size); line-height:var(--panel-line-height); color:var(--color-text-secondary); cursor:pointer; white-space:nowrap; }
  .agent-chat-pane :global(.agent-text-action:hover:not(:disabled)) { color:var(--color-text); text-decoration:underline; }
  .agent-chat-pane :global(.agent-text-action:focus-visible) { outline:2px solid var(--color-focus); outline-offset:1px; }
  .agent-chat-pane :global(.agent-text-action:disabled) { opacity:.5; cursor:default; }
  .agent-chat-pane :global(.queued-note) { margin:0; color:var(--panel-muted); font-size:var(--panel-meta-size); line-height:var(--panel-line-height); }
  .agent-chat-pane :global(.context-trigger) { display:inline-flex; align-items:center; gap:calc(var(--spacing) * .5); min-width:0; max-width:100%; padding:0; border:0; background:transparent; border-radius:var(--radius-base); font:inherit; font-size:var(--panel-meta-size); line-height:var(--panel-line-height); color:var(--panel-muted); cursor:pointer; }
  .agent-chat-pane :global(.context-trigger:hover) { color:var(--color-text); }
  .agent-chat-pane :global(.context-trigger:focus-visible) { outline:2px solid var(--color-focus); outline-offset:1px; }
  .agent-chat-pane :global(.context-label) { min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  @media (pointer: coarse) { .agent-chat-pane :global(.agent-text-action), .agent-chat-pane :global(.context-trigger) { min-height:2.75rem; } }
  .task-prepare { flex:1 1 0; min-height:0; gap:var(--spacing); overflow-y:auto; }
  .task-head { display:flex; align-items:center; gap:var(--spacing); }
  .task-back { display:inline-flex; align-items:center; justify-content:center; padding:calc(var(--spacing) * 1.5); margin-left:calc(var(--spacing) * -1.5); border:0; border-radius:var(--radius-base); background:none; color:inherit; cursor:pointer; }
  .task-back:hover { background-color:var(--color-row-hover); }
  .task-title { margin:0; font-weight:600; min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .task-description { margin:0; font-size:.85em; }
  .task-target { margin:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .task-scope .select, .task-prepare .input { width:100%; min-width:0; }
  .task-extra { field-sizing:content; min-height:2rem; max-height:8rem; overflow:auto; resize:none; }
  .prepare-actions { display:flex; justify-content:flex-end; }
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
  .agent-chat-pane :global(.chat-transcript-wrap) { flex:1 0 8rem; min-height:8rem; }
</style>
