<script>
  // The diagnostics report: what the user hands over when something breaks.
  // The text is built from fields the application chose, shown here in full
  // before it is copied, downloaded or pasted into an issue.
  import { collectReport, hasBuildLog } from "../../lib/support/report.js";
  import { desktop, desktopReport, shellAction } from "../../lib/desktop/client.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { compact = false } = $props();

  const ISSUES_URL = "https://github.com/LibrePaper/librepaper/issues/new";

  let includeBuildLog = $state(false);
  let text = $state("");
  let status = $state("");
  let failed = $state(false);
  let area = $state(null);
  const buildLog = hasBuildLog();
  const inDesktop = Boolean(desktop());

  async function build() {
    let desktopText = "";
    if (desktop()) {
      try { desktopText = await desktopReport(); }
      catch (error) { desktopText = `Desktop app report unavailable: ${error?.message || "unknown error"}`; }
    }
    text = await collectReport({ includeBuildLog, desktopText });
    return text;
  }

  function say(message, bad = false) { status = message; failed = bad; }

  // The clipboard is written in the same task as the click. Safari and
  // WebKitGTK refuse a write that waits for the report first, but they accept
  // a ClipboardItem whose content is still a promise.
  async function copy() {
    const building = build();
    try {
      if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) throw new Error("no ClipboardItem");
      const blob = building.then((report) => new Blob([report], { type: "text/plain" }));
      await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
    } catch {
      const report = await building;
      try {
        await navigator.clipboard.writeText(report);
      } catch (error) {
        area?.select();
        if (!document.execCommand("copy")) throw error;
      }
    }
  }

  async function copyReport() {
    try { await copy(); say(m.diagnosticspanel_copied()); }
    catch (error) { say(error?.message || m.diagnosticspanel_copy_failed(), true); }
  }

  async function download() {
    try {
      const report = await build();
      const link = document.createElement("a");
      const url = URL.createObjectURL(new Blob([report], { type: "text/plain" }));
      link.href = url;
      link.download = `librepaper-diagnostics-${new Date().toISOString().slice(0, 10)}.txt`;
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      say(m.diagnosticspanel_downloaded());
    } catch (error) {
      say(error?.message || m.diagnosticspanel_download_failed(), true);
    }
  }

  // The link opens the issue page itself, in a new tab or, in the desktop app,
  // in the system browser. The click only copies the report.
  async function reportIssue() {
    try {
      await copy();
      say(m.diagnosticspanel_issue_copied());
    } catch (error) {
      say(error?.message || m.diagnosticspanel_copy_failed(), true);
    }
  }

  async function openLogs() {
    try { await shellAction("open-logs"); say(""); }
    catch (error) { say(error?.message || m.diagnosticspanel_desktop_failed(), true); }
  }

  function toggled(event) {
    if (event.currentTarget.open && !text) void build();
  }
</script>

<div class="diagnostics" class:compact>
  <div class="diagnostics-buttons">
    <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void copyReport()}>{m.diagnosticspanel_copy()}</button>
    <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void download()}>{m.diagnosticspanel_download()}</button>
    <a href={ISSUES_URL} target="_blank" rel="noreferrer" class="btn btn-sm lp-control-tonal-brand" onclick={() => void reportIssue()}>{m.diagnosticspanel_report_issue()}</a>
    {#if inDesktop}<button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void openLogs()}>{m.diagnosticspanel_open_logs()}</button>{/if}
  </div>
  <p class="setting-description">{m.diagnosticspanel_local_note()}</p>
  {#if buildLog}
    <label class="diagnostics-check">
      <input type="checkbox" bind:checked={includeBuildLog} />
      <span>{m.diagnosticspanel_build_log()}</span>
    </label>
  {/if}
  {#if status}<p class="setting-description" class:diagnostics-error={failed} role="status">{status}</p>{/if}
  <details class="diagnostics-report" ontoggle={toggled}>
    <summary>{m.diagnosticspanel_show_report()}</summary>
    <textarea class="setting-textarea" readonly rows="14" aria-label={m.diagnosticspanel_report_label()} bind:this={area} value={text}></textarea>
  </details>
</div>

<style>
  .diagnostics { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .diagnostics.compact { gap: calc(var(--spacing) * 1.5); margin-top: 0.75rem; }
  .diagnostics-buttons { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }
  .diagnostics-check { display: flex; align-items: center; gap: calc(var(--spacing) * 2); }
  .diagnostics-error { color: var(--color-error-text); }
  .diagnostics-report summary { cursor: pointer; color: var(--panel-muted); font-size: var(--panel-meta-size); }
  .diagnostics-report textarea { margin-top: calc(var(--spacing) * 2); }
  .compact .diagnostics-report summary, .compact .setting-description { font-size: 0.8125rem; }
</style>
