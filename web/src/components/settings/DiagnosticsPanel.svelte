<script>
  // The diagnostics report: what the user hands over when something breaks.
  // The text is built from fields the application chose, shown here in full
  // before it is copied, downloaded or pasted into an issue.
  import { collectReport, hasBuildLog } from "../../lib/support/report.js";
  import { desktop, desktopReport, shellAction } from "../../lib/desktop/client.js";

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
    try { await copy(); say("Copied."); }
    catch (error) { say(error?.message || "The report could not be copied.", true); }
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
      say("Downloaded.");
    } catch (error) {
      say(error?.message || "The report could not be downloaded.", true);
    }
  }

  // The tab opens before anything is awaited, or a popup blocker stops it.
  async function reportIssue() {
    window.open(ISSUES_URL, "_blank", "noopener");
    try {
      await copy();
      say("The report is copied. Paste it into the issue, or email it to admin@librepaper.org.");
    } catch (error) {
      say(error?.message || "The report could not be copied.", true);
    }
  }

  async function openLogs() {
    try { await shellAction("open-logs"); say(""); }
    catch (error) { say(error?.message || "The desktop app could not do that.", true); }
  }

  function toggled(event) {
    if (event.currentTarget.open && !text) void build();
  }
</script>

<div class="diagnostics" class:compact>
  <div class="diagnostics-buttons">
    <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void copyReport()}>Copy diagnostics</button>
    <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void download()}>Download</button>
    <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void reportIssue()}>Report an issue</button>
    {#if inDesktop}<button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => void openLogs()}>Open logs</button>{/if}
  </div>
  <p class="setting-description">Diagnostics are copied locally. Nothing is sent automatically.</p>
  {#if buildLog}
    <label class="diagnostics-check">
      <input type="checkbox" bind:checked={includeBuildLog} />
      <span>Include the build log (contains document text)</span>
    </label>
  {/if}
  {#if status}<p class="setting-description" class:diagnostics-error={failed} role="status">{status}</p>{/if}
  <details class="diagnostics-report" ontoggle={toggled}>
    <summary>Show the report</summary>
    <textarea class="setting-textarea" readonly rows="14" aria-label="Diagnostics report" bind:this={area} value={text}></textarea>
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
