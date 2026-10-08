<script>
  // Whether the companion is running here, and the ways forward when it is
  // not. It watches the status and never probes when it mounts: opening a
  // settings page must not make the browser ask to reach this computer.
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  let { id = undefined, needs = "" } = $props();

  const INSTALL = "https://librepaper.org/local-app.html";
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
  const hint = $derived(
    connected
      ? ""
      : [needs ? `Needed for ${needs}.` : "", state.hint || ""].filter(Boolean).join(" ")
  );

  let connecting = $state(false);
  let failure = $state("");
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

<div {id} class="companion-status">
  <span class="companion-status-label">Companion</span>
  <span title={hint}><StatusPill label={says} tone={state.tone} accessibleLabel={`LibrePaper Companion: ${says}`} /></span>
  {#if !connected}
    {#if local?.state === "unreachable"}<a class="companion-status-link" href={INSTALL} target="_blank" rel="noreferrer">Install</a>{/if}
    {#if local?.state !== "incompatible"}<button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={() => void connect()}>{connecting ? "Connecting…" : "Connect"}</button>{/if}
    {#if local?.state === "incompatible"}<a class="companion-status-link" href={INSTALL} target="_blank" rel="noreferrer">Update</a>{/if}
  {/if}
  {#if failure}<p class="companion-status-error" role="alert">{failure}</p>{:else if !connected && hint}<p class="companion-status-hint">{hint}</p>{/if}
</div>

<style>
  .companion-status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    font-size: var(--text-sm);
  }
  .companion-status-label { color: var(--color-text-secondary); }
  .companion-status-link { font-size: var(--text-sm); }
  .companion-status-error, .companion-status-hint {
    flex-basis: 100%;
    margin: 0;
    font-size: var(--text-sm);
  }
  .companion-status-error { color: var(--color-error-text); }
  .companion-status-hint { color: var(--color-text-secondary); }
</style>
