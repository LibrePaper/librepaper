<script>
  import SettingRow from "./SettingRow.svelte";
  let { remoteConnected = false, remoteNote = "" } = $props();
  const address = globalThis.location?.origin || "Unavailable";
</script>

<SettingRow id="remote-status" title="Remote connection" description={remoteNote || (remoteConnected
  ? "This server keeps the shared project in sync with its collaborators."
  : "The page retries its connection automatically while it remains open.")}>
  <div class="remote-control">
    <code id="remote-address" class="setting-value">{address}</code>
    <span class="remote-pill" class:remote-offline={!remoteConnected} data-tone={remoteConnected ? "good" : "off"}
          role="status" aria-label={`Remote connection ${remoteConnected ? "connected" : "disconnected"}`}>
      <span class="remote-dot" aria-hidden="true"></span>
      {remoteConnected ? "Connected" : "Disconnected"}
    </span>
  </div>
</SettingRow>

<style>
  .remote-control { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); min-width: 0; }
  #remote-address { overflow-wrap: anywhere; word-break: break-word; }
  .remote-pill { display: inline-flex; align-items: center; gap: calc(var(--spacing) * .75); min-height: 1.75rem; flex: 0 0 auto; padding: 0 calc(var(--spacing) * 2); border: 1px solid var(--color-border); border-radius: 999px; background: var(--color-subtle); color: var(--color-text-secondary); font-size: var(--text-xs); white-space: nowrap; }
  .remote-dot { width: .5rem; height: .5rem; flex: 0 0 .5rem; border-radius: 50%; background: var(--color-success-solid); }
  .remote-offline .remote-dot { background: var(--color-error-solid); }
  @media (max-width: 600px) {
    .remote-control { align-items: flex-end; flex-direction: column; gap: calc(var(--spacing) * .75); }
  }
</style>
