<script>
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import { m } from "../../lib/paraglide/messages.js";
  let { remoteConnected = false, remoteNote = "" } = $props();
  const address = globalThis.location?.origin || m.remotesettings_unavailable();
</script>

<SettingRow id="remote-status" title={m.remotesettings_title()} description={remoteNote || (remoteConnected
  ? m.remotesettings_connected()
  : m.remotesettings_retrying())}>
  <div class="remote-control setting-actions">
    <code id="remote-address" class="setting-value">{address}</code>
    <StatusPill label={remoteConnected ? m.remotesettings_status_connected() : m.remotesettings_status_disconnected()} tone={remoteConnected ? "good" : "neutral"}
      accessibleLabel={remoteConnected ? m.remotesettings_aria_connected() : m.remotesettings_aria_disconnected()} />
  </div>
</SettingRow>

<style>
  .remote-control { display: flex; align-items: center; justify-content: flex-end; gap: calc(var(--spacing) * 2); min-width: 0; }
  #remote-address { overflow-wrap: anywhere; word-break: break-word; }
  @media (max-width: 600px) {
    .remote-control { align-items: flex-end; flex-direction: column; gap: calc(var(--spacing) * .75); }
  }
</style>
