<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import * as control from "../../lib/companion/control.js";
  import * as localBridge from "../../lib/companion/client.js";

  let available = $state(false);
  let viewState = $state(null);
  let error = $state("");
  let loadError = $state("");
  let notice = $state("");
  let pending = $state("");
  let connectError = $state("");
  let agentError = $state("");
  let agentNotice = $state("");
  let epoch = 0;
  let requestId = 0;
  let scope = "";

  const list = (value) => Array.isArray(value) ? value : [];
  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());
  const id = (value) => encodeURIComponent(String(value));
  const customAgentIds = $derived(new Set(list(viewState?.custom_agents).map((agent) => String(agent.id))));
  const detectedAgents = $derived(list(viewState?.agents).filter((agent) => !customAgentIds.has(String(agent.id))));

  async function load(expectedScope = scope, expectedEpoch = epoch) {
    if (!available) return;
    const current = ++requestId;
    try {
      const result = await control.request("/state");
      if (current !== requestId || expectedEpoch !== epoch || expectedScope !== control.scope() || !control.available()) return;
      viewState = result;
      loadError = "";
    } catch (cause) {
      if (current === requestId && expectedEpoch === epoch && expectedScope === control.scope()) loadError = cause?.message || "Could not load this computer's settings.";
    }
  }

  async function act(key, success, path, options = {}) {
    if (!available || pending) return;
    const expectedScope = scope;
    const expectedEpoch = epoch;
    pending = key;
    error = "";
    notice = "";
    try {
      await control.request(path, options);
      if (expectedEpoch !== epoch || expectedScope !== control.scope() || !control.available()) return;
      notice = success;
      await load(expectedScope, expectedEpoch);
      return true;
    } catch (cause) {
      if (expectedEpoch === epoch && expectedScope === control.scope()) error = cause?.message || "The request could not be completed.";
      return false;
    } finally {
      if (expectedEpoch === epoch) pending = "";
    }
  }

  async function manageThisComputer() {
    if (pending) return;
    pending = "connect";
    connectError = "";
    try {
      await control.connect(localBridge.address());
      control.showSettings();
    } catch (cause) {
      connectError = cause?.message || "Could not connect to this computer's companion.";
    } finally {
      pending = "";
    }
  }

  async function addAgent(event) {
    event.preventDefault();
    if (pending) return;
    const form = event.currentTarget;
    const data = new FormData(form);
    const label = String(data.get("label") || "").trim();
    const executable = String(data.get("command") || "").trim();
    const args = String(data.get("args") || "").split("\n").map((part) => part.trim()).filter(Boolean);
    agentError = "";
    agentNotice = "";
    if (!label || !executable) {
      agentError = "Enter a name and executable.";
      return;
    }
    const saved = await act("agent-add", "Agent added.", "/agents", {
      method: "POST", body: { label, command: [executable, ...args] },
    });
    if (saved) {
      form.reset();
      agentNotice = "Agent added.";
    } else {
      agentError = error || "Could not add this agent.";
    }
  }

  onMount(() => {
    let alive = true;
    const updateAccess = (access) => {
      if (!alive) return;
      const nextScope = access?.scope || control.scope();
      if (scope !== nextScope || available !== Boolean(access?.available)) {
        scope = nextScope;
        available = Boolean(access?.available);
        epoch++;
        requestId++;
        viewState = null;
        error = "";
        loadError = "";
        notice = "";
        connectError = "";
        agentError = "";
        agentNotice = "";
        pending = "";
      }
      if (available) void load(scope, epoch);
    };
    const unsubscribe = control.subscribe(updateAccess);
    const timer = setInterval(() => { if (available) void load(scope, epoch); }, 5000);
    const visibility = () => { if (document.visibilityState === "visible" && available) void load(scope, epoch); };
    document.addEventListener("visibilitychange", visibility);
    return () => {
      alive = false;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", visibility);
      unsubscribe?.();
      requestId++;
    };
  });
</script>

{#if available}
  {#if error}<p class="setting-description management-error" role="alert">{error}</p>{/if}
  {#if loadError}<p class="setting-description management-error" role="alert">{loadError}</p>{/if}
  {#if notice}<p class="setting-description management-notice" role="status">{notice}</p>{/if}
  {#if !viewState && !error && !loadError}<p class="setting-description">Loading settings for this computer…</p>{/if}

  <section id="local-status" class="settings-subsection" aria-labelledby="companion-lifecycle-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-lifecycle-heading">General</h4>{#if viewState?.version}<span class="setting-description">Version {viewState.version}</span>{/if}</div>
    <SettingRow id="local-startup" title="Start at login" description="Open the companion when you log in to this computer.">
      <button type="button" role="switch" class="switch" class:checked={viewState?.settings?.startup_enabled === true} data-state={viewState?.settings?.startup_enabled == null ? "unknown" : viewState.settings.startup_enabled ? "checked" : "unchecked"} aria-label="Start at login" aria-checked={viewState?.settings?.startup_enabled === true} disabled={!viewState || typeof viewState.settings?.startup_enabled !== "boolean" || Boolean(pending)} onclick={() => void act("startup", "Startup preference saved.", "/settings", { method: "PUT", body: { startup_enabled: !viewState.settings.startup_enabled } })}>
        <span class="switch-thumb" data-state={viewState?.settings?.startup_enabled == null ? "unknown" : viewState.settings.startup_enabled ? "checked" : "unchecked"}></span>
      </button>
    </SettingRow>
    {#if viewState?.standalone}<SettingRow title="Quit companion" description="Close the companion running on this computer.">
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm("Quit LibrePaper companion? Connected sites will no longer reach local tools until it is started again.")) void act("quit", "Quit request sent.", "/quit", { method: "POST" }); }}>Quit companion</button>
    </SettingRow>{/if}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-approvals-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-approvals-heading">Approvals</h4></div>
    {#each list(viewState?.approvals) as approval (approval.id)}
      <SettingRow title={approval.title || "Approval request"} description={approval.message || "This request has no additional details."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`approval-${approval.id}`, "Request denied.", `/approvals/${id(approval.id)}`, { method: "POST", body: { decision: "deny" } })}>Deny</button>
        <button class="btn btn-sm lp-control-brand" type="button" disabled={Boolean(pending)} onclick={() => void act(`approval-${approval.id}`, "Request allowed.", `/approvals/${id(approval.id)}`, { method: "POST", body: { decision: "allow" } })}>{approval.allow_label || "Allow"}</button>
      </SettingRow>
    {:else}<p class="setting-description">No requests are waiting.</p>{/each}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-sites-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-sites-heading">Connected sites</h4></div>
    {#each list(viewState?.pairings) as pairing (pairing.id)}
      <SettingRow title={pairing.origin || "Connected site"} description={pairing.created_at ? `Connected ${new Date(Number(pairing.created_at) * 1000).toLocaleString()}` : "This site can use the companion."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm(`Revoke access for ${pairing.origin}?`)) void act(`pairing-${pairing.id}`, "Site access revoked.", `/pairings/${id(pairing.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {:else}<p class="setting-description">No connected sites.</p>{/each}
  </section>

  {#if list(viewState?.bindings).length}
  <section class="settings-subsection" aria-labelledby="companion-folders-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-folders-heading">Project folders</h4></div>
    <p class="setting-description">Folders a connected site may use on this computer.</p>
    {#each list(viewState?.bindings) as binding (binding.id)}
      <SettingRow title={binding.project || "Authorized folder"} description={`${binding.origin || "Connected site"} · ${binding.entrypoint || "project folder"}${binding.root ? ` · ${binding.root}` : ""}${binding.execution_granted === false ? " · Code execution is not authorized" : ""}`}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) void act(`binding-${binding.id}`, "Folder authorization removed.", `/bindings/${id(binding.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {/each}
  </section>
  {/if}

  <section class="settings-subsection" aria-labelledby="companion-activity-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-activity-heading">Activity</h4></div>
    {#each list(viewState?.jobs) as job (job.id)}
      <SettingRow title={[job.kind, job.stage].filter(Boolean).join(" · ") || "Local job"} description={`${job.status || "running"}${job.error ? ` · ${job.error}` : ""}${job.log_tail ? `\n${Array.isArray(job.log_tail) ? job.log_tail.join("\n") : job.log_tail}` : ""}`} stacked>
        {#if active(job.status)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`job-${job.id}`, "Cancellation requested.", `/jobs/${id(job.id)}/cancel`, { method: "POST" })}>Cancel job</button>{/if}
      </SettingRow>
    {/each}
    {#each list(viewState?.previews) as preview (preview.id)}
      <SettingRow title={preview.label || preview.project || "Preview"} description={`${preview.status || "active"}${preview.log_tail ? `\n${preview.log_tail}` : ""}`} stacked>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`preview-${preview.id}`, "Preview stopped.", `/previews/${id(preview.id)}`, { method: "DELETE" })}>Stop preview</button>
      </SettingRow>
    {/each}
    {#each list(viewState?.sessions) as session (session.id)}
      {@const sessionStatus = session.status || session.state || "active"}
      <SettingRow title={session.name || session.agent || session.id || "Agent session"} description={`${sessionStatus}${session.task || session.task_id || session.detail ? ` · ${session.task || session.task_id || session.detail}` : ""}`}>
        {#if active(sessionStatus)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`session-${session.id}`, "Agent stop requested.", `/agents/sessions/${id(session.id)}/cancel`, { method: "POST" })}>Stop</button>{/if}
      </SettingRow>
    {/each}
    {#if !list(viewState?.jobs).length && !list(viewState?.previews).length && !list(viewState?.sessions).length}<p class="setting-description">Nothing is running.</p>{/if}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-agents-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-agents-heading">Agents on this computer</h4></div>
    {#each detectedAgents as agent (agent.id)}
      <SettingRow title={agent.label || agent.id || "Detected agent"} description={agent.assistant_blocked || (agent.assistant ? "Available for assistant sessions." : "Detected on this computer.")}>
        {#if agent.assistant_blocked}<span class="setting-description management-error">{agent.assistant_blocked}</span>{/if}
      </SettingRow>
    {/each}
    {#each list(viewState?.custom_agents) as agent (agent.id)}
      <SettingRow title={agent.label || agent.id || "Configured agent"} description={list(agent.command).join(" ")}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm(`Remove ${agent.label || "this agent"}?`)) void act(`agent-${agent.id}`, "Agent removed.", `/agents/${id(agent.id)}`, { method: "DELETE" }); }}>Remove</button>
      </SettingRow>
    {/each}
    {#if !detectedAgents.length && !list(viewState?.custom_agents).length}<p class="setting-description">No agents are detected or configured.</p>{/if}
    <SettingRow title="Add an agent" description="Add a command that LibrePaper can run on this computer." stacked>
      {#if agentError}<p class="setting-description management-error" role="alert">{agentError}</p>{/if}
      {#if agentNotice}<p class="setting-description management-notice" role="status">{agentNotice}</p>{/if}
      <p class="setting-description">Enter the executable separately from its arguments. Put one argument on each line.</p>
      <form class="management-agent-form" onsubmit={addAgent}>
        <label class="management-field">Name<input class="input input-sm setting-input" name="label" required disabled={Boolean(pending)} /></label>
        <label class="management-field">Executable<input class="input input-sm setting-input" name="command" placeholder="npx" required disabled={Boolean(pending)} /></label>
        <label class="management-field">Arguments<textarea class="input input-sm setting-input" name="args" rows="2" placeholder="One argument per line" disabled={Boolean(pending)}></textarea></label>
        <button class="btn btn-sm lp-control-outline" type="submit" disabled={Boolean(pending)}>{pending === "agent-add" ? "Adding…" : "Add agent"}</button>
      </form>
    </SettingRow>
  </section>

{:else}
  <section class="settings-subsection management-unavailable">
    <div class="settings-section-title"><h4 class="settings-subhead">Manage this computer</h4></div>
    <SettingRow title="Computer controls" description="Connect to the companion running on this computer to manage approvals, connected sites, project folders, activity, and agents.">
      {#if connectError}<span class="setting-description management-error" role="alert">{connectError}</span>{/if}
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void manageThisComputer()}>{pending === "connect" ? "Connecting…" : "Manage this computer"}</button>
    </SettingRow>
  </section>
{/if}

<style>
  .management-error { color: var(--color-error-text); }
  .management-notice { color: var(--color-success-text); }
  .management-agent-form { display: grid; gap: calc(var(--spacing) * 2); width: min(100%, 46rem); }
  .management-field { display: grid; gap: calc(var(--spacing) * .75); font-size: var(--text-sm); }
  .management-unavailable { margin-top: calc(var(--spacing) * 3); }
</style>
