<script>
  // The one line every companion-backed setting starts with: what needs the
  // companion, whether it is running, and the two ways forward (install it,
  // connect to it). The light matches the Local pill in the navbar.
  //
  // It watches the status and never probes when it mounts: opening a settings
  // page must not make the browser ask to reach other apps on this computer.
  // Connect is the gesture that may.
  import * as localBridge from "../../lib/companion/client.js";
  import * as control from "../../lib/companion/control.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  // `needs` names what depends on the companion here, such as "Zotero".
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
  // What needs it, then what to do about it while it is not there.
  const note = $derived([needs ? `Needed for ${needs}.` : "", connected ? "" : state.hint || ""].filter(Boolean).join(" "));

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

{#if managedAvailable}
  <div class="companion-block" data-tone="good" {id}>
    <span class="companion-dot" aria-hidden="true"></span>
    <div class="companion-words"><div class="companion-line"><span class="companion-name">Companion</span><span class="companion-state" role="status">Management access available</span></div><div class="companion-note">Connected sites and local tools are managed in Settings → Companion.</div></div>
  </div>
{:else}
<div class="companion-block" data-tone={state.tone} {id}>
  <span class="companion-dot" aria-hidden="true"></span>
  <div class="companion-words">
    <div class="companion-line">
      <span class="companion-name">Companion</span>
      <span class="companion-state" role="status" aria-label={`LibrePaper Companion: ${says}`}>{says}</span>
    </div>
    {#if failure}<div class="companion-note error" role="alert">{failure}</div>
    {:else if note}<div class="companion-note">{note}</div>{/if}
  </div>
  {#if !connected}
    <div class="companion-actions">
      {#if local?.state !== "denied"}<a class="btn btn-sm lp-control-outline" href={INSTALL} target="_blank" rel="noreferrer">Install</a>{/if}
      {#if local?.state !== "incompatible"}<button type="button" class="btn btn-sm lp-control-brand" disabled={connecting} onclick={() => void connect()}>{connecting ? "Connecting…" : "Connect"}</button>{/if}
    </div>
  {/if}
</div>
{/if}

<style>
  .companion-block { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: center; gap: calc(var(--spacing) * 2.5); min-height: calc(var(--spacing) * 11); margin-block: calc(var(--spacing) * 2) calc(var(--spacing) * 3); padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 3); border: 1px solid var(--color-divider); border-radius: var(--radius-container); background: var(--color-subtle); }
  .companion-dot { width: .5rem; height: .5rem; flex: 0 0 .5rem; border-radius: 50%; background: var(--color-border-strong); }
  [data-tone="good"] .companion-dot { background: var(--color-success-solid); }
  [data-tone="warn"] .companion-dot { background: var(--color-warning-solid); }
  [data-tone="error"] .companion-dot { background: var(--color-error-solid); }
  .companion-words { min-width: 0; }
  .companion-line { display: flex; align-items: baseline; gap: calc(var(--spacing) * 2); font-size: var(--text-sm); }
  .companion-name { font-weight: 500; }
  .companion-state { color: var(--color-text-secondary); }
  .companion-note { color: var(--panel-muted); font-size: var(--panel-meta-size); overflow-wrap: anywhere; }
  .companion-note.error { color: var(--color-error-text); }
  .companion-actions { display: flex; gap: calc(var(--spacing) * 1.5); }
  .companion-actions :is(.btn) { min-height: calc(var(--spacing) * 7.5); height: calc(var(--spacing) * 7.5); }
  @media (max-width: 600px) {
    .companion-block { grid-template-columns: auto minmax(0, 1fr); }
    .companion-actions { grid-column: 2; }
  }
</style>
