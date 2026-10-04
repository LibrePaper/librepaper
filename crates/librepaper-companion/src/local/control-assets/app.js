(() => {
  "use strict";

  // The link carries a private control token only in its fragment. Remove it
  // before anything can navigate or copy the URL, then keep it per tab.
  const storageKey = "librepaper-companion-control";
  let token = "";
  try {
    const fragment = new URLSearchParams(location.hash.slice(1));
    const offered = fragment.get("token");
    if (offered) {
      token = offered;
      try { sessionStorage.setItem(storageKey, offered); } catch (_) { /* token remains in memory for this tab */ }
    } else {
      token = sessionStorage.getItem(storageKey) || "";
    }
  } catch (_) {
    token = "";
  }
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);

  const $ = (id) => document.getElementById(id);
  const locked = $("locked");
  const dashboard = $("dashboard");
  const notice = $("notice");
  let lastState = null;
  let settingsDirty = false;
  let refreshInFlight = false;
  let noticeTimer = 0;
  let pollingTimer = 0;
  const rendered = new Map();

  function renderWhenChanged(key, value, renderValue) {
    const signature = JSON.stringify(value);
    if (rendered.get(key) === signature) return;
    rendered.set(key, signature);
    renderValue(value);
  }

  function node(tag, className, text) {
    const item = document.createElement(tag);
    if (className) item.className = className;
    if (text !== undefined && text !== null) item.textContent = String(text);
    return item;
  }

  function setConnected(connected, label) {
    $("connection-dot").classList.toggle("offline", !connected);
    $("connection-label").textContent = label;
  }

  function showNotice(message, kind = "info") {
    clearTimeout(noticeTimer);
    notice.textContent = message;
    notice.className = `notice ${kind}`;
    notice.hidden = false;
    noticeTimer = setTimeout(() => { notice.hidden = true; }, 7000);
  }

  async function api(path, options = {}) {
    if (!token) throw new Error("This dashboard link has expired. Open the dashboard from LibrePaper again.");
    const headers = new Headers(options.headers || {});
    headers.set("Authorization", `Bearer ${token}`);
    if (options.body !== undefined) headers.set("Content-Type", "application/json");
    let response;
    try {
      response = await fetch(`/companion/api${path}`, { ...options, headers, cache: "no-store" });
    } catch (_) {
      throw new Error("LibrePaper could not be reached. Check that the companion is running.");
    }
    if (response.status === 401 || response.status === 403) {
      try { sessionStorage.removeItem(storageKey); } catch (_) { /* storage may be disabled */ }
      token = "";
      throw new Error("This dashboard link is no longer valid. Open the dashboard from LibrePaper again.");
    }
    let body = null;
    const contentType = response.headers.get("content-type") || "";
    if (contentType.includes("application/json")) {
      try { body = await response.json(); } catch (_) { body = null; }
    }
    if (!response.ok) {
      const message = body && typeof body.error === "string" ? body.error : `The request failed (${response.status}).`;
      throw new Error(message);
    }
    return body;
  }

  function safeId(id) { return encodeURIComponent(String(id)); }
  function list(value) { return Array.isArray(value) ? value : []; }
  function replaceContents(target, children, emptyText) {
    target.replaceChildren(...children.length ? children : [node("div", "empty panel", emptyText)]);
  }
  function actionButton(label, className, run, ariaLabel) {
    const button = node("button", `button ${className || "quiet"}`, label);
    button.type = "button";
    if (ariaLabel) button.setAttribute("aria-label", ariaLabel);
    button.addEventListener("click", run);
    return button;
  }
  async function act(label, callback) {
    try {
      await callback();
      if (label) showNotice(label, "success");
      await refresh();
    } catch (error) {
      showNotice(error.message || "The request could not be completed.", "error");
      if (!token) lockDashboard();
    }
  }

  function renderApprovals(approvals) {
    $("approval-count").textContent = String(approvals.length);
    const cards = approvals.map((approval) => {
      const card = node("article", "request-card");
      const content = node("div", "request-content");
      content.append(node("h3", "", approval.title || "Approval request"));
      const message = node("p", "request-message", approval.message || "This request has no additional details.");
      content.append(message);
      const actions = node("div", "button-row");
      const id = safeId(approval.id);
      const deny = actionButton("Deny", "quiet", () => act("Request denied.", () => api(`/approvals/${id}`, { method: "POST", body: JSON.stringify({ decision: "deny" }) })));
      deny.dataset.focusKey = `deny-approval-${approval.id}`;
      const allow = actionButton(approval.allow_label || "Allow", "primary", () => act("Request allowed.", () => api(`/approvals/${id}`, { method: "POST", body: JSON.stringify({ decision: "allow" }) })));
      allow.dataset.focusKey = `allow-approval-${approval.id}`;
      actions.append(deny, allow);
      card.append(content, actions);
      return card;
    });
    replaceContents($("approvals"), cards, "No requests are waiting.");
  }

  function toolItems(capabilities) {
    const tools = capabilities && capabilities.tools && typeof capabilities.tools === "object" ? capabilities.tools : {};
    const items = Object.entries(tools).map(([name, tool]) => [name, tool]);
    if (capabilities && capabilities.calepin) items.push(["Calepin", capabilities.calepin]);
    if (capabilities && capabilities.zotero) items.push(["Zotero", capabilities.zotero]);
    if (capabilities && capabilities.quarto && capabilities.quarto.tool) items.push(["Quarto", capabilities.quarto.tool]);
    for (const builder of list(capabilities && capabilities.builders)) {
      if (builder && builder.id) items.push([builder.id, builder]);
    }
    const byName = new Map();
    for (const [name, tool] of items) {
      const key = name.toLowerCase();
      const existing = byName.get(key);
      if (!existing || (!existing[1].available && tool.available) || (!existing[1].version && tool.version)) {
        byName.set(key, [name, tool]);
      }
    }
    return [...byName.values()];
  }

  function renderTools(capabilities) {
    const tools = toolItems(capabilities);
    const cards = tools.map(([name, tool]) => {
      const available = Boolean(tool && tool.available);
      const card = node("article", "tool-card");
      const heading = node("div", "tool-heading");
      heading.append(node("h3", "", name.charAt(0).toUpperCase() + name.slice(1)));
      const badge = node("span", `badge ${available ? "good" : "muted"}`, available ? "Available" : "Not found");
      heading.append(badge);
      card.append(heading);
      if (tool && tool.version) card.append(node("p", "version", tool.version));
      const help = tool && tool.note ? tool.note : available ? "Ready to use from LibrePaper." : `Install ${name} on this computer, then rescan tools.`;
      card.append(node("p", "tool-note", help));
      return card;
    });
    const confinement = capabilities && capabilities.confinement;
    if (confinement) {
      const card = node("article", "tool-card");
      card.append(node("div", "tool-heading"), node("h3", "", "Execution environment"));
      card.append(node("p", "tool-note", confinement.available ? `Available (${confinement.kind || "local"}).` : confinement.reason || "Runs with your user account permissions."));
      cards.push(card);
    }
    replaceContents($("tools"), cards, "Tool status is not available yet.");
  }

  function renderPairings(pairings) {
    const cards = pairings.map((pairing) => {
      const card = node("article", "list-card");
      const body = node("div", "list-card-body");
      body.append(node("h3", "", pairing.origin || "Connected site"));
      body.append(node("p", "subtle", pairing.created_at ? `Connected ${new Date(Number(pairing.created_at) * 1000).toLocaleString()}` : "This site can use the companion."));
      const revoke = actionButton("Revoke", "quiet", () => {
        if (confirm(`Revoke LibrePaper access for ${pairing.origin || "this site"}?`)) return act("Site access revoked.", () => api(`/pairings/${safeId(pairing.id)}`, { method: "DELETE" }));
      }, `Revoke access for ${pairing.origin || "site"}`);
      revoke.dataset.focusKey = `revoke-pairing-${pairing.id}`;
      card.append(body, revoke);
      return card;
    });
    replaceContents($("pairings"), cards, "No connected sites.");
  }

  function renderBindings(bindings) {
    const cards = bindings.map((binding) => {
      const card = node("article", "list-card");
      const body = node("div", "list-card-body");
      body.append(node("h3", "", binding.project || "Authorized folder"));
      body.append(node("p", "subtle", `${binding.origin || "Connected site"} · ${binding.entrypoint || "project folder"}`));
      if (binding.root) body.append(node("p", "command", binding.root));
      if (binding.execution_granted === false) body.append(node("small", "subtle", "Code execution is not authorized."));
      const revoke = actionButton("Revoke", "quiet", () => {
        if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) return act("Folder authorization removed.", () => api(`/bindings/${safeId(binding.id)}`, { method: "DELETE" }));
      }, `Revoke folder authorization for ${binding.project || "folder"}`);
      revoke.dataset.focusKey = `revoke-binding-${binding.id}`;
      card.append(body, revoke);
      return card;
    });
    replaceContents($("bindings"), cards, "No folders are authorized.");
  }

  function isActive(status) { return !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase()); }
  function renderActivity(state) {
    const rows = [];
    for (const job of list(state.jobs)) {
      const card = node("article", "activity-card");
      const top = node("div", "activity-top");
      const title = [job.kind, job.stage].filter(Boolean).join(" · ") || "Local job";
      top.append(node("h3", "", title), node("span", "badge muted", job.status || "running"));
      card.append(top);
      if (job.log_tail) {
        const details = node("details", "log-details");
        details.dataset.key = `job-${job.id}`;
        details.append(node("summary", "", "Recent output"));
        const output = Array.isArray(job.log_tail) ? job.log_tail.join("\n") : String(job.log_tail);
        details.append(node("pre", "log-output", output.slice(-12000)));
        card.append(details);
      }
      if (job.error) card.append(node("p", "error-text", job.error));
      if (isActive(job.status)) {
        const cancel = actionButton("Cancel job", "quiet", () => act("Cancellation requested.", () => api(`/jobs/${safeId(job.id)}/cancel`, { method: "POST" })));
        cancel.dataset.focusKey = `cancel-job-${job.id}`;
        card.append(cancel);
      }
      rows.push(card);
    }
    for (const preview of list(state.previews)) {
      const card = node("article", "activity-card");
      card.append(node("div", "activity-top", preview.label || preview.project || "Preview"), node("p", "subtle", preview.status || "active"));
      if (preview.error) card.append(node("p", "error-text", preview.error));
      if (preview.log_tail) card.append(node("pre", "log-output", String(preview.log_tail).slice(-12000)));
      const stop = actionButton("Stop preview", "quiet", () => act("Preview stopped.", () => api(`/previews/${safeId(preview.id)}`, { method: "DELETE" })));
      stop.dataset.focusKey = `stop-preview-${preview.id}`;
      card.append(stop);
      rows.push(card);
    }
    for (const session of list(state.sessions)) {
      const card = node("article", "activity-card");
      const label = session.name || session.agent || session.id || "Agent session";
      const sessionStatus = session.status || session.state || "active";
      card.append(node("div", "activity-top", label), node("p", "subtle", sessionStatus));
      if (session.task || session.task_id || session.detail) card.append(node("p", "subtle", session.task || session.task_id || session.detail));
      if (session.error) card.append(node("p", "error-text", session.error));
      if (session.log_tail) card.append(node("pre", "log-output", String(session.log_tail).slice(-12000)));
      if (isActive(sessionStatus)) {
        const stop = actionButton("Stop agent", "quiet", () => act("Agent stop requested.", () => api(`/agents/sessions/${safeId(session.id)}/cancel`, { method: "POST" })));
        stop.dataset.focusKey = `stop-agent-${session.id}`;
        card.append(stop);
      }
      rows.push(card);
    }
    replaceContents($("activity"), rows, "Nothing is running.");
  }

  function field(label, input, help) {
    const wrap = node("label", "field");
    wrap.append(node("span", "", label), input);
    if (help) wrap.append(node("small", "", help));
    return wrap;
  }

  function renderSettings(settings) {
    const editingSettings = $("settings-form").contains(document.activeElement);
    if (!settingsDirty && !editingSettings) {
      $("tool-paths").value = list(settings.tool_paths).join("\n");
      $("startup-enabled").checked = Boolean(settings.startup_enabled);
      $("startup-setting").hidden = typeof settings.startup_enabled !== "boolean";
      $("tray-enabled").checked = Boolean(settings.tray_enabled);
      $("tray-setting").hidden = typeof settings.tray_enabled !== "boolean";
      $("tray-help").textContent = settings.tray_available === false
        ? "No tray service is available in this desktop session. You can still manage LibrePaper here or from the command line."
        : settings.tray_available === true
          ? "Available in this desktop session. The setting changes take effect immediately."
          : "Keep quick controls available in the system tray when supported.";
      const integrations = Array.isArray(settings.integrations)
        ? settings.integrations
        : Object.entries(settings.integrations || {}).map(([name, integration]) => ({ name, ...integration }));
      const cards = integrations.map((integration) => {
        const name = String(integration.name || "tool");
        const section = node("section", "integration-card");
        section.dataset.integration = name;
        section.append(node("h3", "", name.charAt(0).toUpperCase() + name.slice(1)));
        const path = node("input"); path.type = "text"; path.autocomplete = "off"; path.spellcheck = false;
        path.value = integration.path || ""; path.dataset.setting = "path";
        const args = node("textarea"); args.rows = 2; args.spellcheck = false; args.value = list(integration.args).join("\n"); args.dataset.setting = "args";
        section.append(field("Executable path (optional)", path, "Leave blank to use the companion's normal tool lookup."), field("Extra arguments", args, "One argument per line."));
        return section;
      });
      $("integration-settings").replaceChildren(...cards);
      for (const input of $("integration-settings").querySelectorAll("input, textarea")) input.addEventListener("input", markSettingsDirty, { once: true });
    }
    const agents = list(settings.custom_agents || (lastState && lastState.custom_agents));
    const customIds = new Set(agents.map((agent) => String(agent.id)));
    const agentCards = agents.map((agent) => {
      const card = node("article", "list-card");
      const body = node("div", "list-card-body");
      body.append(node("h3", "", agent.label || agent.id || "Configured agent"));
      body.append(node("p", "command", list(agent.command).join(" ")));
      const remove = actionButton("Remove", "quiet", () => {
        if (confirm(`Remove ${agent.label || "this agent"} from the companion?`)) return act("Agent removed.", () => api(`/agents/${safeId(agent.id)}`, { method: "DELETE" }));
      }, `Remove ${agent.label || "agent"}`);
      remove.dataset.focusKey = `remove-agent-${agent.id}`;
      card.append(body, remove);
      return card;
    });
    for (const agent of list(lastState && lastState.agents)) {
      if (customIds.has(String(agent.id))) continue;
      const card = node("article", "list-card");
      const body = node("div", "list-card-body");
      body.append(node("h3", "", agent.label || agent.id || "Installed agent"));
      body.append(node("p", agent.assistant_blocked ? "error-text" : "subtle", agent.assistant_blocked || (agent.assistant ? "Available for assistant sessions." : "Detected on this computer.")));
      card.append(body);
      agentCards.push(card);
    }
    const agentSignature = JSON.stringify({ custom: agents, detected: list(lastState && lastState.agents) });
    if (rendered.get("agents") !== agentSignature) {
      rendered.set("agents", agentSignature);
      replaceContents($("agents"), agentCards, "No agents are configured.");
    }
  }

  function markSettingsDirty() { settingsDirty = true; }

  function render(state) {
    const focusKey = document.activeElement && document.activeElement.dataset ? document.activeElement.dataset.focusKey : "";
    lastState = state;
    renderWhenChanged("approvals", list(state.approvals).map(({ id, title, message, allow_label }) => ({ id, title, message, allow_label })), () => renderApprovals(list(state.approvals)));
    renderWhenChanged("tools", state.tools || {}, renderTools);
    renderWhenChanged("pairings", list(state.pairings), renderPairings);
    renderWhenChanged("bindings", list(state.bindings), renderBindings);
    const activeFocusKey = document.activeElement && document.activeElement.dataset ? document.activeElement.dataset.focusKey : "";
    const openDetails = new Set([...$("activity").querySelectorAll("details[open]")].map((details) => details.dataset.key));
    const activitySignature = JSON.stringify({ jobs: state.jobs, previews: state.previews, sessions: state.sessions });
    if (rendered.get("activity") !== activitySignature) {
      rendered.set("activity", activitySignature);
      renderActivity(state);
      for (const details of $("activity").querySelectorAll("details")) details.open = openDetails.has(details.dataset.key);
      if (activeFocusKey) {
        const next = [...$("activity").querySelectorAll("[data-focus-key]")].find((item) => item.dataset.focusKey === activeFocusKey);
        if (next) next.focus();
      }
    }
    const settings = state.settings || {};
    renderSettings(settings);
    if (focusKey && document.activeElement === document.body) {
      const next = [...dashboard.querySelectorAll("[data-focus-key]")].find((item) => item.dataset.focusKey === focusKey);
      if (next) next.focus();
    }
    $("version-label").textContent = `LibrePaper ${state.version || "companion"}`;
    $("standalone-note").textContent = state.standalone ? "Standalone companion" : "Managed by another LibrePaper process";
    $("quit-button").hidden = !state.standalone;
    $("welcome-line").textContent = `Local tools and access for LibrePaper · ${state.tools && state.tools.platform ? state.tools.platform : "this computer"}`;
  }

  function lockDashboard() {
    dashboard.hidden = true;
    locked.hidden = false;
    setConnected(false, "Not connected");
  }

  async function refresh() {
    if (!token || refreshInFlight || document.visibilityState === "hidden") return;
    refreshInFlight = true;
    try {
      const state = await api("/state", { method: "GET" });
      if (!state || typeof state !== "object") throw new Error("LibrePaper returned an unreadable status response.");
      locked.hidden = true;
      dashboard.hidden = false;
      render(state);
      setConnected(true, "Companion is running");
    } catch (error) {
      setConnected(false, "Connection unavailable");
      showNotice(error.message || "Could not load companion status.", "error");
      if (!token) lockDashboard();
    } finally {
      refreshInFlight = false;
    }
  }

  $("refresh-button").addEventListener("click", refresh);
  $("rescan-button").addEventListener("click", () => act("Tool scan complete.", () => api("/tools/rescan", { method: "POST" })));
  $("settings-form").addEventListener("input", markSettingsDirty);
  $("settings-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const integrations = Object.fromEntries([...$("integration-settings").children].map((section) => [section.dataset.integration, {
      path: section.querySelector('[data-setting="path"]').value.trim() || null,
      args: section.querySelector('[data-setting="args"]').value.split("\n").map((value) => value.trim()).filter(Boolean),
    }]));
    const body = {
      tool_paths: $("tool-paths").value.split("\n").map((value) => value.trim()).filter(Boolean),
      integrations,
    };
    if (lastState && lastState.settings && typeof lastState.settings.startup_enabled === "boolean") body.startup_enabled = $("startup-enabled").checked;
    if (lastState && lastState.settings && typeof lastState.settings.tray_enabled === "boolean") body.tray_enabled = $("tray-enabled").checked;
    try {
      await api("/settings", { method: "PUT", body: JSON.stringify(body) });
      settingsDirty = false;
      showNotice("Settings saved.", "success");
      await refresh();
    } catch (error) { showNotice(error.message || "Settings could not be saved.", "error"); }
  });
  $("agent-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const body = {
      label: $("agent-name").value.trim(),
      command: [$("agent-command").value.trim(), ...$("agent-args").value.split("\n").map((value) => value.trim()).filter(Boolean)],
    };
    try {
      await api("/agents", { method: "POST", body: JSON.stringify(body) });
      event.target.reset();
      showNotice("Agent added.", "success");
      await refresh();
    } catch (error) { showNotice(error.message || "Agent could not be added.", "error"); }
  });
  $("quit-button").addEventListener("click", async () => {
    if (!confirm("Quit LibrePaper companion? Connected sites will no longer reach local tools until it is started again.")) return;
    try {
      await api("/quit", { method: "POST" });
      clearInterval(pollingTimer);
      setConnected(false, "Companion is stopping");
      showNotice("Quit request sent. You can close this tab.", "success");
    } catch (error) { showNotice(error.message || "The companion could not quit.", "error"); }
  });
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") refresh(); });

  if (token) {
    dashboard.hidden = false;
    locked.hidden = true;
    refresh();
    pollingTimer = setInterval(refresh, 2000);
  } else {
    lockDashboard();
  }
})();
