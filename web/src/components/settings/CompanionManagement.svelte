<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import * as control from "../../lib/companion/control.js";

  let available = $state(false);
  let state = $state(null);
  let error = $state("");
  let notice = $state("");
  let pending = $state("");
  let toolPaths = $state("");
  let toolPathsDirty = $state(false);
  let project = $state("");
  let entrypoint = $state("index.qmd");
  let selectedOrigin = $state("");
  let epoch = 0;
  let requestId = 0;
  let scope = "";

  const list = (value) => Array.isArray(value) ? value : [];
  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());
  const id = (value) => encodeURIComponent(String(value));

  async function load(expectedScope = scope, expectedEpoch = epoch) {
    if (!available) return;
    const current = ++requestId;
    try {
      const result = await control.request("/state");
      if (current !== requestId || expectedEpoch !== epoch || expectedScope !== control.scope() || !control.available()) return;
      state = result;
      error = "";
      if (!toolPathsDirty) toolPaths = list(result?.settings?.tool_paths).join("\n");
    } catch (cause) {
      if (current === requestId && expectedEpoch === epoch && expectedScope === control.scope()) error = cause?.message || "Could not load this computer's settings.";
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

  async function saveToolPaths() {
    const paths = toolPaths.split("\n").map((value) => value.trim()).filter(Boolean);
    await act("tool-paths", "Tool search folders saved.", "/settings", { method: "PUT", body: { tool_paths: paths } });
    if (!error) toolPathsDirty = false;
  }

  async function addBinding() {
    const pairing = list(state?.pairings).find((item) => item.origin === selectedOrigin);
    if (!pairing) {
      error = "Connect a site before authorizing one of its project folders.";
      return;
    }
    const currentProject = project.trim();
    const currentEntrypoint = entrypoint.trim();
    await act("folder", "Folder authorization added.", "/bindings/folder", {
      method: "POST",
      body: { origin: pairing.origin, project: currentProject, entrypoint: currentEntrypoint },
    });
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
        state = null;
        error = "";
        notice = "";
        pending = "";
        toolPathsDirty = false;
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
  {#if notice}<p class="setting-description management-notice" role="status">{notice}</p>{/if}
  {#if !state && !error}<p class="setting-description">Loading settings for this computer…</p>{/if}

  <section class="settings-subsection" aria-labelledby="companion-lifecycle-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-lifecycle-heading">Companion</h4></div>
    <SettingRow id="managed-startup" title="Start at login" description="Open the companion when you log in to this computer.">
      <button type="button" role="switch" class="switch" class:checked={state?.settings?.startup_enabled === true} data-state={state?.settings?.startup_enabled == null ? "unknown" : state.settings.startup_enabled ? "checked" : "unchecked"} aria-label="Start at login" aria-checked={state?.settings?.startup_enabled === true} disabled={!state || typeof state.settings?.startup_enabled !== "boolean" || Boolean(pending)} onclick={() => void act("startup", "Startup preference saved.", "/settings", { method: "PUT", body: { startup_enabled: !state.settings.startup_enabled } })}>
        <span class="switch-thumb" data-state={state?.settings?.startup_enabled == null ? "unknown" : state.settings.startup_enabled ? "checked" : "unchecked"}></span>
      </button>
    </SettingRow>
    {#if state?.standalone}<SettingRow title="Quit companion" description="Close the companion running on this computer.">
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm("Quit LibrePaper companion? Connected sites will no longer reach local tools until it is started again.")) void act("quit", "Quit request sent.", "/quit", { method: "POST" }); }}>Quit companion</button>
    </SettingRow>{/if}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-approvals-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-approvals-heading">Approvals</h4></div>
    {#each list(state?.approvals) as approval (approval.id)}
      <SettingRow title={approval.title || "Approval request"} description={approval.message || "This request has no additional details."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`approval-${approval.id}`, "Request denied.", `/approvals/${id(approval.id)}`, { method: "POST", body: { decision: "deny" } })}>Deny</button>
        <button class="btn btn-sm lp-control-brand" type="button" disabled={Boolean(pending)} onclick={() => void act(`approval-${approval.id}`, "Request allowed.", `/approvals/${id(approval.id)}`, { method: "POST", body: { decision: "allow" } })}>{approval.allow_label || "Allow"}</button>
      </SettingRow>
    {:else}<p class="setting-description">No requests are waiting.</p>{/each}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-sites-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-sites-heading">Connected sites</h4></div>
    {#each list(state?.pairings) as pairing (pairing.id)}
      <SettingRow title={pairing.origin || "Connected site"} description={pairing.created_at ? `Connected ${new Date(Number(pairing.created_at) * 1000).toLocaleString()}` : "This site can use the companion."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm(`Revoke access for ${pairing.origin}?`)) void act(`pairing-${pairing.id}`, "Site access revoked.", `/pairings/${id(pairing.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {:else}<p class="setting-description">No connected sites.</p>{/each}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-folders-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-folders-heading">Authorized folders</h4></div>
    {#each list(state?.bindings) as binding (binding.id)}
      <SettingRow title={binding.project || "Authorized folder"} description={`${binding.origin || "Connected site"} · ${binding.entrypoint || "project folder"}${binding.root ? ` · ${binding.root}` : ""}${binding.execution_granted === false ? " · Code execution is not authorized" : ""}`}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) void act(`binding-${binding.id}`, "Folder authorization removed.", `/bindings/${id(binding.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {:else}<p class="setting-description">No folders are authorized.</p>{/each}
    <SettingRow title="Authorize a project folder" description={list(state?.pairings).length ? "Choose the site and project, then select a folder in the companion." : "Connect a site before granting folder access."} stacked>
      <div class="management-folder-form">
        <label class="management-field">Connected site<select class="input input-sm setting-input" bind:value={selectedOrigin} disabled={!list(state?.pairings).length || Boolean(pending)}><option value="">Choose a site</option>{#each list(state?.pairings) as pairing (pairing.id)}<option value={pairing.origin}>{pairing.origin}</option>{/each}</select></label>
        <label class="management-field">Project name<input class="input input-sm setting-input" bind:value={project} placeholder="Project" disabled={!list(state?.pairings).length || Boolean(pending)} /></label>
        <label class="management-field">Entry file<input class="input input-sm setting-input" bind:value={entrypoint} placeholder="index.qmd" disabled={!list(state?.pairings).length || Boolean(pending)} /></label>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={!selectedOrigin || !project.trim() || !entrypoint.trim() || Boolean(pending)} onclick={() => void addBinding()}>{pending === "folder" ? "Choosing…" : "Choose folder"}</button>
      </div>
    </SettingRow>
  </section>

  <section class="settings-subsection" aria-labelledby="companion-activity-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-activity-heading">Activity</h4></div>
    {#each list(state?.jobs) as job (job.id)}
      <SettingRow title={[job.kind, job.stage].filter(Boolean).join(" · ") || "Local job"} description={`${job.status || "running"}${job.error ? ` · ${job.error}` : ""}${job.log_tail ? `\n${Array.isArray(job.log_tail) ? job.log_tail.join("\n") : job.log_tail}` : ""}`} stacked>
        {#if active(job.status)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`job-${job.id}`, "Cancellation requested.", `/jobs/${id(job.id)}/cancel`, { method: "POST" })}>Cancel job</button>{/if}
      </SettingRow>
    {/each}
    {#each list(state?.previews) as preview (preview.id)}
      <SettingRow title={preview.label || preview.project || "Preview"} description={`${preview.status || "active"}${preview.log_tail ? `\n${preview.log_tail}` : ""}`} stacked>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`preview-${preview.id}`, "Preview stopped.", `/previews/${id(preview.id)}`, { method: "DELETE" })}>Stop preview</button>
      </SettingRow>
    {/each}
    {#each list(state?.sessions) as session (session.id)}
      {@const sessionStatus = session.status || session.state || "active"}
      <SettingRow title={session.name || session.agent || session.id || "Agent session"} description={`${sessionStatus}${session.task || session.task_id || session.detail ? ` · ${session.task || session.task_id || session.detail}` : ""}`}>
        {#if active(sessionStatus)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => void act(`session-${session.id}`, "Agent stop requested.", `/agents/sessions/${id(session.id)}/cancel`, { method: "POST" })}>Stop</button>{/if}
      </SettingRow>
    {/each}
    {#if !list(state?.jobs).length && !list(state?.previews).length && !list(state?.sessions).length}<p class="setting-description">Nothing is running.</p>{/if}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-agents-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-agents-heading">Agents on this computer</h4></div>
    {#each list(state?.agents) as agent (agent.id)}
      <SettingRow title={agent.label || agent.id || "Detected agent"} description={agent.assistant_blocked || (agent.assistant ? "Available for assistant sessions." : "Detected on this computer.")}>
        {#if agent.assistant_blocked}<span class="setting-description management-error">{agent.assistant_blocked}</span>{/if}
      </SettingRow>
    {/each}
    {#each list(state?.custom_agents) as agent (agent.id)}
      <SettingRow title={agent.label || agent.id || "Configured agent"} description={list(agent.command).join(" ")}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(pending)} onclick={() => { if (confirm(`Remove ${agent.label || "this agent"}?`)) void act(`agent-${agent.id}`, "Agent removed.", `/agents/${id(agent.id)}`, { method: "DELETE" }); }}>Remove</button>
      </SettingRow>
    {/each}
    {#if !list(state?.agents).length && !list(state?.custom_agents).length}<p class="setting-description">No agents are detected or configured.</p>{/if}
    <SettingRow title="Add an agent" description="Add a command that LibrePaper can run on this computer." stacked>
      <form class="management-agent-form" onsubmit={(event) => {
        event.preventDefault();
        const form = event.currentTarget;
        const data = new FormData(form);
        const label = String(data.get("label") || "").trim();
        const command = [String(data.get("command") || "").trim(), ...String(data.get("args") || "").split("\n").map((part) => part.trim()).filter(Boolean)].filter(Boolean);
        if (!label || !command.length) return;
        void act("agent-add", "Agent added.", "/agents", { method: "POST", body: { label, command } }).then((saved) => { if (saved) form.reset(); });
      }}>
        <label class="management-field">Name<input class="input input-sm setting-input" name="label" required /></label>
        <label class="management-field">Command<input class="input input-sm setting-input" name="command" required /></label>
        <label class="management-field">Arguments<textarea class="input input-sm setting-input" name="args" rows="2" /></label>
        <button class="btn btn-sm lp-control-outline" type="submit" disabled={Boolean(pending)}>{pending === "agent-add" ? "Adding…" : "Add agent"}</button>
      </form>
    </SettingRow>
  </section>

  <section class="settings-subsection" aria-labelledby="companion-tool-paths-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-tool-paths-heading">Tool search folders</h4></div>
    <SettingRow title="Extra folders" description="Search these folders before the usual PATH. One absolute folder path per line." stacked>
      <textarea class="input setting-input management-paths" aria-label="Extra tool search folders" rows="3" bind:value={toolPaths} oninput={() => toolPathsDirty = true} disabled={!state || Boolean(pending)}></textarea>
      <button class="btn btn-sm lp-control-brand" type="button" disabled={!state || !toolPathsDirty || Boolean(pending)} onclick={() => void saveToolPaths()}>{pending === "tool-paths" ? "Saving…" : "Save folders"}</button>
    </SettingRow>
  </section>
{:else}
  <section class="settings-subsection management-unavailable">
    <div class="settings-section-title"><h4 class="settings-subhead">Manage this computer</h4></div>
    <SettingRow title="Computer controls" description="Choose Settings from the LibrePaper tray menu to manage approvals, connected sites, authorized folders, activity, agents, and local tool settings. The button can open Settings when this app has a registered link handler.">
      <button class="btn btn-sm lp-control-outline" type="button" onclick={() => control.openSettings()}>Manage this computer</button>
    </SettingRow>
  </section>
{/if}

<style>
  .management-error { color: var(--color-error-text); }
  .management-notice { color: var(--color-success-text); }
  .management-folder-form, .management-agent-form { display: grid; gap: calc(var(--spacing) * 2); width: min(100%, 46rem); }
  .management-field { display: grid; gap: calc(var(--spacing) * .75); font-size: var(--text-sm); }
  .management-paths { width: min(100%, 46rem); }
  .management-unavailable { margin-top: calc(var(--spacing) * 3); }
</style>
