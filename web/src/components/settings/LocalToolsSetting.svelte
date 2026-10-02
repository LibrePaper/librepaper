<script>
  // Whether the companion is there to run the local renderers. It watches the
  // status and never probes when it mounts: opening the Render page must not
  // make the browser ask to reach other apps on this computer. Choosing a
  // local renderer does, and so do the buttons.
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const label = $derived(({ unknown: "Not checked", unreachable: "Disconnected", denied: "Access blocked", reachable: "Needs approval", unauthorized: "Needs approval", incompatible: "Update needed", connected: "Connected" })[local?.state] || "Not checked");
  const tone = $derived(({ denied: "error", unauthorized: "warn", incompatible: "warn", reachable: "warn", connected: "good" })[local?.state] || "neutral");
  const message = $derived(local?.state === "denied" || local?.state === "unreachable" ? local.instructions : ({ unknown: "Calepin, Pandoc and Quarto run through the companion.", reachable: "Connect this site to use local tools.", unauthorized: "Reconnect this site to use local tools.", incompatible: "Update the companion to use local tools.", connected: "" })[local?.state] || "");

  async function rescan() { try { await localBridge.capabilities({ rescan: true }); } catch { /* status explains failure */ } }
  async function connect() { try { await localBridge.connectApp(); } catch { /* local status carries instructions */ } }
</script>

<SettingRow id="render-local" title="Local tools" scope="This computer" description={message}>
  <StatusPill {label} {tone} />
  {#if ["unreachable", "denied", "unauthorized", "reachable"].includes(local?.state)}<button type="button" class="btn btn-sm lp-control-brand" onclick={connect}>Connect</button>{/if}
  {#if local?.state !== "connected"}<a class="btn btn-sm lp-control-outline" href="https://librepaper.org/install.html" target="_blank" rel="noreferrer">Install companion</a>{/if}
  <button type="button" class="btn btn-sm lp-control-outline" disabled={local?.state !== "connected"} onclick={rescan}>Rescan</button>
</SettingRow>
