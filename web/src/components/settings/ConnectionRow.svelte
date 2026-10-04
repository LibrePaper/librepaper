<script>
  // Whether the companion is running here, and the ways forward when it is
  // not. It watches the status and never probes when it mounts: opening a
  // settings page must not make the browser ask to reach this computer.
  import SettingRow from "./SettingRow.svelte";
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import * as control from "../../lib/companion/control.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { id = undefined, needs = "" } = $props();

  const INSTALL = "https://librepaper.org/install.html";
  const STATES = {
    connected: { says: "Running", tone: "good" },
    unreachable: { says: "Not running", tone: "error", hint: "Run librepaper in a terminal, or install it." },
    denied: { says: "Blocked", tone: "error", hint: "Allow local network access for this site in the browser's site settings." },
    reachable: { says: "Needs approval", tone: "warn", hint: "Connect, then approve the dialog the companion shows." },
    unauthorized: { says: "Needs approval", tone: "warn", hint: "Connect, then approve the dialog the companion shows." },
    incompatible: { says: "Update needed", tone: "warn", hint: "This companion is too old for this site. Install the latest version." },
    unknown: { says: "Not checked", tone: "neutral" },
  };

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const state = $derived(STATES[local?.state] || STATES.unknown);
  const connected = $derived(local?.state === "connected");
  const says = $derived(connected && local?.version ? `${state.says} · ${local.version}` : state.says);
  const description = $derived(
    managedAvailable || connected
      ? "Runs local programs for LibrePaper on this computer."
      : [needs ? `Needed for ${needs}.` : "", state.hint || ""].filter(Boolean).join(" ")
  );

  let connecting = $state(false);
  let failure = $state("");
  let managedAvailable = $state(control.available());
  $effect(() => {
    const unsubscribe = control.subscribe((access) => { managedAvailable = Boolean(access?.available); });
    return unsubscribe;
  });
  $effect(() => { if (connected) failure = ""; });

  async function connect() {
    if (connecting) return;
    connecting = true;
    failure = "";
    try { await localBridge.connectApp(); }
    catch (error) { failure = error?.message || "Could not reach the companion."; }
    finally { connecting = false; }
  }
</script>

<SettingRow {id} title="Companion" {description}>
  <div class="setting-actions">
    {#if managedAvailable}
      <StatusPill label={local?.version ? `Connected · ${local.version}` : "Connected"} tone="good" />
    {:else}
      <StatusPill label={says} tone={state.tone} accessibleLabel={`LibrePaper Companion: ${says}`} />
      {#if !connected}
        {#if local?.state !== "denied"}<a class="btn btn-sm lp-control-outline" href={INSTALL} target="_blank" rel="noreferrer">Install</a>{/if}
        {#if local?.state !== "incompatible"}<button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={() => void connect()}>{connecting ? "Connecting…" : "Connect"}</button>{/if}
      {/if}
    {/if}
  </div>
  {#if failure}
    <span class="setting-description connection-error" role="alert">{failure}</span>
  {/if}
</SettingRow>

<style>
  .connection-error { color: var(--color-error-text); }
</style>
