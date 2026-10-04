<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { createMachineView } from "../../lib/companion/machine.svelte.js";

  const view = createMachineView();
  const { list, id } = view;

  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());

  onMount(() => view.start());
</script>

{#if view.available}
  {#if view.error}
    <p class="setting-description management-error" role="alert">{view.error}</p>
  {/if}
  {#if view.loadError}
    <p class="setting-description management-error" role="alert">{view.loadError}</p>
  {/if}
  {#if view.notice}
    <p class="setting-description management-notice" role="status">{view.notice}</p>
  {/if}
  {#if !view.state && !view.error && !view.loadError}
    <p class="setting-description">Loading settings for this computer…</p>
  {/if}

  {#if list(view.state?.approvals).length}
  <section class="settings-subsection" aria-labelledby="companion-approvals-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-approvals-heading">Waiting for your answer</h4></div>
    <p class="setting-description">The companion asks before a site connects to this computer or changes its settings.</p>
    {#each list(view.state?.approvals) as approval (approval.id)}
      <SettingRow title={approval.title || "Approval request"} description={approval.message || "This request has no additional details."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`approval-${approval.id}`, "Request denied.", `/approvals/${id(approval.id)}`, { method: "POST", body: { decision: "deny" } })}>Deny</button>
        <button class="btn btn-sm lp-control-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`approval-${approval.id}`, "Request allowed.", `/approvals/${id(approval.id)}`, { method: "POST", body: { decision: "allow" } })}>{approval.allow_label || "Allow"}</button>
      </SettingRow>
    {/each}
  </section>
  {/if}

  <section id="local-status" class="settings-subsection" aria-labelledby="companion-lifecycle-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-lifecycle-heading">General</h4>{#if view.state?.version}<span class="setting-description">Version {view.state.version}</span>{/if}</div>
    {#if view.state?.standalone === true}
    <SettingRow id="local-startup" title="Start at login" description="Open the companion when you log in.">
      <button type="button" role="switch" class="switch" class:checked={view.state?.settings?.startup_enabled === true} data-state={view.state?.settings?.startup_enabled == null ? "unknown" : view.state.settings.startup_enabled ? "checked" : "unchecked"} aria-label="Start at login" aria-checked={view.state?.settings?.startup_enabled === true} disabled={!view.state || typeof view.state.settings?.startup_enabled !== "boolean" || Boolean(view.pending)} onclick={() => void view.act("startup", "Startup preference saved.", "/settings", { method: "PUT", body: { startup_enabled: !view.state.settings.startup_enabled } })}>
        <span class="switch-thumb" data-state={view.state?.settings?.startup_enabled == null ? "unknown" : view.state.settings.startup_enabled ? "checked" : "unchecked"}></span>
      </button>
    </SettingRow>
    {/if}
    {#if view.state?.standalone}
    <SettingRow title="Quit" description="Close the companion on this computer.">
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm("Quit LibrePaper companion? Connected sites will no longer reach local tools until it is started again.")) void view.act("quit", "Quit request sent.", "/quit", { method: "POST" }); }}>Quit companion</button>
    </SettingRow>
    {/if}
  </section>

  <section class="settings-subsection" aria-labelledby="companion-sites-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-sites-heading">Connected sites</h4></div>
    {#each list(view.state?.pairings) as pairing (pairing.id)}
      <SettingRow title={pairing.origin || "Connected site"} description={pairing.created_at ? `Connected ${new Date(Number(pairing.created_at) * 1000).toLocaleString()}` : "This site can use the companion."}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Revoke access for ${pairing.origin}?`)) void view.act(`pairing-${pairing.id}`, "Site access revoked.", `/pairings/${id(pairing.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {:else}
    <p class="setting-description">None.</p>
    {/each}
  </section>

  {#if list(view.state?.bindings).length}
  <section class="settings-subsection" aria-labelledby="companion-folders-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-folders-heading">Project folders</h4></div>
    {#each list(view.state?.bindings) as binding (binding.id)}
      <SettingRow title={binding.project || "Authorized folder"} description={`${binding.origin || "Connected site"} · ${binding.entrypoint || "project folder"}${binding.root ? ` · ${binding.root}` : ""}${binding.execution_granted === false ? " · Code execution is not authorized" : ""}`}>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) void view.act(`binding-${binding.id}`, "Folder authorization removed.", `/bindings/${id(binding.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {/each}
  </section>
  {/if}

  <section class="settings-subsection" aria-labelledby="companion-activity-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="companion-activity-heading">Activity</h4></div>
    {#each list(view.state?.jobs) as job (job.id)}
      <SettingRow title={[job.kind, job.stage].filter(Boolean).join(" · ") || "Local job"} description={`${job.status || "running"}${job.error ? ` · ${job.error}` : ""}${job.log_tail ? `\n${Array.isArray(job.log_tail) ? job.log_tail.join("\n") : job.log_tail}` : ""}`} stacked>
        {#if active(job.status)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`job-${job.id}`, "Cancellation requested.", `/jobs/${id(job.id)}/cancel`, { method: "POST" })}>Cancel job</button>{/if}
      </SettingRow>
    {/each}
    {#each list(view.state?.previews) as preview (preview.id)}
      <SettingRow title={preview.label || preview.project || "Preview"} description={`${preview.status || "active"}${preview.log_tail ? `\n${preview.log_tail}` : ""}`} stacked>
        <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`preview-${preview.id}`, "Preview stopped.", `/previews/${id(preview.id)}`, { method: "DELETE" })}>Stop preview</button>
      </SettingRow>
    {/each}
    {#each list(view.state?.sessions) as session (session.id)}
      {@const sessionStatus = session.status || session.state || "active"}
      <SettingRow title={session.name || session.agent || session.id || "Agent session"} description={`${sessionStatus}${session.task || session.task_id || session.detail ? ` · ${session.task || session.task_id || session.detail}` : ""}`}>
        {#if active(sessionStatus)}<button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`session-${session.id}`, "Agent stop requested.", `/agents/sessions/${id(session.id)}/cancel`, { method: "POST" })}>Stop</button>{/if}
      </SettingRow>
    {/each}
    {#if !list(view.state?.jobs).length && !list(view.state?.previews).length && !list(view.state?.sessions).length}
    <p class="setting-description">Nothing running.</p>
    {/if}
  </section>

{:else}
  <section class="settings-subsection management-unavailable">
    <SettingRow title="This computer" description="Connect to manage this computer's companion.">
      {#if view.connectError}<span class="setting-description management-error" role="alert">{view.connectError}</span>{/if}
      <button class="btn btn-sm lp-control-outline" type="button" disabled={Boolean(view.pending)} onclick={() => void view.manageThisComputer()}>{view.pending === "connect" ? "Connecting…" : "Manage this computer"}</button>
    </SettingRow>
  </section>
{/if}

<style>
  .management-error { color: var(--color-error-text); }
  .management-notice { color: var(--color-success-text); }
  .management-unavailable { margin-top: calc(var(--spacing) * 3); }
</style>
