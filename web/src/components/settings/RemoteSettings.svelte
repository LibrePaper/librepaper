<script>
  import SettingRow from "./SettingRow.svelte";
  let { remoteConnected = false, remoteNote = "" } = $props();
  const address = globalThis.location?.origin || "Unavailable";
</script>

<div id="remote-status" class="setting-status" class:remote-offline={!remoteConnected} data-tone={remoteConnected ? "good" : "off"}>
  <span class="setting-status-dot" aria-hidden="true"></span>
  <div class="setting-status-words">
    <div class="setting-title" role="status">{remoteConnected ? "Connected to the LibrePaper server." : "Not connected to the LibrePaper server."}</div>
    <div class="setting-description">
      {remoteNote || (remoteConnected
        ? "This server keeps the shared project in sync with its collaborators."
        : "The page retries its connection automatically while it remains open. Reconnect status appears here.")}
    </div>
  </div>
</div>

<SettingRow id="remote-address" title="Server address" description="The server this project is using. Server selection is managed by the project link.">
  <code class="setting-value">{address}</code>
</SettingRow>

<p class="setting-description">
  The remote server stores and synchronizes this shared project. The Local page describes the companion and services available on this computer.
</p>

<style>
  .remote-offline :global(.setting-title) { color: var(--color-error-500); }
  .remote-offline :global(.setting-status-dot) { background-color: var(--color-error-500); }
  :global(#remote-address code) { overflow-wrap: anywhere; word-break: break-word; }
</style>
