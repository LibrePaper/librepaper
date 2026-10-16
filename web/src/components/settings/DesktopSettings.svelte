<script>
  import SettingRow from "./SettingRow.svelte";
  import DesktopOnly from "../DesktopOnly.svelte";
  import { capabilities, shellAction } from "../../lib/desktop/client.js";

  let { view } = $props();
  const active = (status) => !["done", "complete", "completed", "succeeded", "failed", "error", "cancelled", "canceled", "stopped", "interrupted", "expired", "denied"].includes(String(status || "").toLowerCase());

  let report = $state("");
  async function checkSetup() {
    try {
      report = JSON.stringify(await capabilities({ rescan: true }), null, 2);
    } catch (error) {
      report = error?.message || "The setup check could not reach the desktop app.";
    }
  }

  let shellStatus = $state("");
  let shellFailed = $state(false);
  async function runShell(name, done) {
    shellStatus = ""; shellFailed = false;
    try { await shellAction(name); shellStatus = done; }
    catch (error) { shellFailed = true; shellStatus = error?.message || "The desktop app could not do that."; }
  }
</script>

{#if view.error}
  <p class="setting-description desktop-error" role="alert">{view.error}</p>
{/if}
{#if view.loadError}
  <p class="setting-description desktop-error" role="alert">{view.loadError}</p>
{/if}
{#if view.notice}
  <p class="setting-description desktop-notice" role="status">{view.notice}</p>
{/if}

<DesktopOnly>
{#if view.available}
  {#if view.list(view.state?.bindings).length}
  <section id="desktop-folders" class="settings-subsection" aria-labelledby="desktop-folders-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="desktop-folders-heading">Project folders</h4></div>
    {#each view.list(view.state?.bindings) as binding (binding.id)}
      <SettingRow title={binding.project || "Authorized folder"} description={`${binding.origin || "Connected site"} · ${binding.entrypoint || "project folder"}${binding.root ? ` · ${binding.root}` : ""}${binding.execution_granted === false ? " · Code execution is not authorized" : ""}`}>
        <button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => { if (confirm(`Remove folder authorization for ${binding.project || "this project"}?`)) void view.act(`binding-${binding.id}`, "Folder authorization removed.", `/bindings/${view.id(binding.id)}`, { method: "DELETE" }); }}>Revoke</button>
      </SettingRow>
    {/each}
  </section>
  {/if}

  {#if view.state?.version}
  <section id="desktop-version" class="settings-subsection" aria-labelledby="desktop-version-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="desktop-version-heading">Desktop app</h4><span class="setting-description">Version {view.state.version}</span></div>
  </section>
  {/if}

  <section id="desktop-report" class="settings-subsection" aria-labelledby="desktop-report-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="desktop-report-heading">Check local setup</h4></div>
    <SettingRow title="Tool report" description="Rescan the tools the desktop app found on this computer and view the report.">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void checkSetup()}>Check</button>
    </SettingRow>
    {#if report}<pre class="setting-log" role="status">{report}</pre>{/if}
  </section>

  <section id="desktop-diagnostics" class="settings-subsection" aria-labelledby="desktop-diagnostics-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="desktop-diagnostics-heading">Diagnostics</h4></div>
    <SettingRow title="Copy diagnostics" description="Copies the app version, your system, the tools found and the recent log, to paste into a bug report. Nothing is sent anywhere.">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void runShell("diagnostics", "Diagnostics copied.")}>Copy</button>
    </SettingRow>
    <SettingRow title="Log folder" description="Open the folder that holds the desktop app's log.">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void runShell("open-logs", "")}>Open</button>
    </SettingRow>
    {#if shellStatus}<p class="setting-description" class:desktop-error={shellFailed} role="status">{shellStatus}</p>{/if}
  </section>

  <section id="desktop-activity" class="settings-subsection" aria-labelledby="desktop-activity-heading">
    <div class="settings-section-title"><h4 class="settings-subhead" id="desktop-activity-heading">Activity</h4></div>
    {#each view.list(view.state?.jobs) as job (job.id)}
      <SettingRow title={[job.kind, job.stage].filter(Boolean).join(" · ") || "Local job"} description={`${job.status || "running"}${job.error ? ` · ${job.error}` : ""}${job.log_tail ? `\n${Array.isArray(job.log_tail) ? job.log_tail.join("\n") : job.log_tail}` : ""}`} stacked>
        {#if active(job.status)}<button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`job-${job.id}`, "Cancellation requested.", `/jobs/${view.id(job.id)}/cancel`, { method: "POST" })}>Cancel</button>{/if}
      </SettingRow>
    {/each}
    {#each view.list(view.state?.previews) as preview (preview.id)}
      <SettingRow title={preview.label || preview.project || "Preview"} description={`${preview.status || "active"}${preview.log_tail ? `\n${preview.log_tail}` : ""}`} stacked>
        <button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`preview-${preview.id}`, "Preview stopped.", `/previews/${view.id(preview.id)}`, { method: "DELETE" })}>Stop preview</button>
      </SettingRow>
    {/each}
    {#each view.list(view.state?.sessions) as session (session.id)}
      {@const sessionStatus = session.status || session.state || "active"}
      <SettingRow title={session.name || session.agent || session.id || "Agent session"} description={`${sessionStatus}${session.task || session.task_id || session.detail ? ` · ${session.task || session.task_id || session.detail}` : ""}`}>
        {#if active(sessionStatus)}<button class="btn btn-sm lp-control-tonal-brand" type="button" disabled={Boolean(view.pending)} onclick={() => void view.act(`session-${session.id}`, "Agent stop requested.", `/agents/sessions/${view.id(session.id)}/cancel`, { method: "POST" })}>Stop</button>{/if}
      </SettingRow>
    {/each}
    {#if !view.list(view.state?.jobs).length && !view.list(view.state?.previews).length && !view.list(view.state?.sessions).length}
    <p class="setting-description">Nothing running.</p>
    {/if}
  </section>
{/if}
</DesktopOnly>

<p class="setting-description desktop-help">Still stuck? <a href="https://github.com/LibrePaper/librepaper/issues" target="_blank" rel="noreferrer">Report a problem</a>.</p>

<style>
  .desktop-error { color: var(--color-error-text); }
  .desktop-notice { color: var(--color-success-text); }
  .desktop-help { margin-top: calc(var(--spacing) * 3); }
</style>
