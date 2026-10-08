<script>
  // Whether the companion is running here, and the ways forward when it is
  // not. It watches the status and never probes when it mounts: opening a
  // settings page must not make the browser ask to reach this computer.
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";
  import { stateOf } from "../../lib/companion/states.js";

  let { id = undefined, needs = "" } = $props();

  const INSTALL = "https://librepaper.org/local-app.html";

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const wording = $derived(stateOf(local));
  const connected = $derived(local?.state === "connected");
  const says = $derived(wording.says);
  const hint = $derived(
    connected
      ? ""
      : [needs ? `Needed for ${needs}.` : "", wording.hint].filter(Boolean).join(" ")
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

<div {id} class="companion-connection">
  <span title={hint}><StatusPill label={says} tone={wording.tone} accessibleLabel={`LibrePaper Companion: ${says}`} /></span>
  {#if !connected}
    {#if local?.state !== "denied"}<a class="companion-connection-link" href={INSTALL} target="_blank" rel="noreferrer">Install</a>{/if}
    {#if local?.state !== "incompatible"}<button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={() => void connect()}>{connecting ? "Connecting…" : "Connect"}</button>{/if}
  {/if}
  {#if failure}<p class="companion-connection-error" role="alert">{failure}</p>{:else if !connected && hint}<p class="companion-connection-hint">{hint}</p>{/if}
</div>

<style>
  .companion-connection {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    margin-block: calc(var(--spacing) * 3) calc(var(--spacing) * 2);
    font-size: var(--text-sm);
  }
  .companion-connection-link { font-size: var(--text-sm); }
  .companion-connection-error, .companion-connection-hint {
    flex-basis: 100%;
    margin: 0;
    font-size: var(--text-sm);
  }
  .companion-connection-error { color: var(--color-error-text); }
  .companion-connection-hint { color: var(--color-text-secondary); }
</style>
