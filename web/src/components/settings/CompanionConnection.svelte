<script>
  // Whether the desktop app is available, and the ways forward when it is not.
  // Outside the desktop app it is the one notice that every companion feature
  // shows. Inside, it shows the status and a Retry. It watches the status and
  // never probes when it mounts.
  import StatusPill from "./StatusPill.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { companion } from "../../lib/companion/status.svelte.js";
  import { INSTALL_URL, stateOf } from "../../lib/companion/states.js";

  let { id = undefined, needs = "" } = $props();

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  const wording = $derived(stateOf(local));
  const ready = $derived(local?.state === "ready");
  const inBrowser = $derived(local?.state === "browser");
  const hint = $derived(
    [needs ? `Needed for ${needs}.` : "", wording.hint].filter(Boolean).join(" ")
  );
  const openLink = $derived(`librepaper://open?url=${encodeURIComponent(globalThis.location?.href || "")}`);

  let retrying = $state(false);
  async function retry() {
    if (retrying) return;
    retrying = true;
    try { await localBridge.retry(); }
    finally { retrying = false; }
  }
</script>

<!-- A widget that names a need disappears once the need is met. -->
{#if !(needs && ready)}
<div {id} class="companion-connection">
  {#if inBrowser}
    <p class="companion-connection-hint">Computing, agents, folders and backups need the LibrePaper desktop app.</p>
    <a class="btn btn-sm lp-control-tonal-brand" href={INSTALL_URL} target="_blank" rel="noreferrer">Install the desktop app</a>
    <a class="btn btn-sm lp-control-brand" href={openLink}>Open in the desktop app</a>
  {:else}
    <span title={hint}><StatusPill label={wording.says} tone={wording.tone} accessibleLabel={`LibrePaper desktop app: ${wording.says}`} /></span>
    {#if local?.state === "unavailable"}<button type="button" class="btn btn-sm lp-control-brand" disabled={retrying} onclick={() => void retry()}>{retrying ? "Retrying…" : "Retry"}</button>{/if}
    {#if local?.state === "incompatible"}<a class="btn btn-sm lp-control-tonal-brand" href={INSTALL_URL} target="_blank" rel="noreferrer">Install the desktop app</a>{/if}
    {#if !ready && hint}<p class="companion-connection-hint">{hint}</p>{/if}
  {/if}
</div>
{/if}

<style>
  .companion-connection {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    margin-block: calc(var(--spacing) * 3) calc(var(--spacing) * 2);
    font-size: var(--text-sm);
  }
  .companion-connection-hint {
    flex-basis: 100%;
    margin: 0;
    font-size: var(--text-sm);
  }
  .companion-connection-hint { color: var(--color-text-secondary); }
</style>
