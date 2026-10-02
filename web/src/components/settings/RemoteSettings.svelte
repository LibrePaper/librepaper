<script>
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  let { remoteConnected = false, remoteNote = "" } = $props();
  const address = globalThis.location?.origin || "Unavailable";
</script>

<SettingRow id="remote-status" title="Remote connection" description={remoteNote || (remoteConnected
  ? "This server syncs the shared project with collaborators."
  : "Connection retries while this page is open.")}>
  <div class="remote-control setting-actions">
    <code id="remote-address" class="setting-value">{address}</code>
    <StatusPill label={remoteConnected ? "Connected" : "Disconnected"} tone={remoteConnected ? "good" : "neutral"}
      accessibleLabel={`Remote connection ${remoteConnected ? "connected" : "disconnected"}`} />
  </div>
</SettingRow>

<style>
  .remote-control { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); min-width: 0; }
  #remote-address { overflow-wrap: anywhere; word-break: break-word; }
  @media (max-width: 600px) {
    .remote-control { align-items: flex-end; flex-direction: column; gap: calc(var(--spacing) * .75); }
  }
</style>
