<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import * as control from "../../lib/companion/control.js";
  import { companion } from "../../lib/companion/status.svelte.js";

  const local = $derived(companion.status);
  $effect(() => companion.watch());
  let managed = $state(null);
  let managedAvailable = $state(control.available());
  let busy = $state(false);
  let error = $state("");
  let notice = $state("");
  let epoch = 0;
  let sequence = 0;
  let scope = "";

  function items(capabilities) {
    const tools = capabilities && capabilities.tools && typeof capabilities.tools === "object" ? capabilities.tools : {};
    const entries = Object.entries(tools);
    if (capabilities?.calepin) entries.push(["Calepin", capabilities.calepin]);
    if (capabilities?.zotero) entries.push(["Zotero", capabilities.zotero]);
    if (capabilities?.quarto?.tool) entries.push(["Quarto", capabilities.quarto.tool]);
    for (const builder of Array.isArray(capabilities?.builders) ? capabilities.builders : []) if (builder?.id) entries.push([builder.id, builder]);
    const unique = new Map();
    for (const [name, tool] of entries) {
      const key = name.toLowerCase();
      const old = unique.get(key);
      if (!old || (!old.tool?.available && tool?.available) || (!old.tool?.version && tool?.version)) unique.set(key, { name, tool });
    }
    return [...unique.values()];
  }

  async function load(expectedScope = scope, expectedEpoch = epoch) {
    if (!managedAvailable) return;
    const request = ++sequence;
    try {
      const result = await control.request("/state");
      if (request === sequence && expectedEpoch === epoch && expectedScope === control.scope() && control.available()) { managed = result; error = ""; }
    } catch (cause) {
      if (request === sequence && expectedEpoch === epoch && expectedScope === control.scope()) error = cause?.message || "Tool status could not be loaded.";
    }
  }

  async function rescan() {
    if (busy) return;
    const expectedScope = scope;
    const expectedEpoch = epoch;
    busy = true;
    error = "";
    notice = "";
    try {
      await control.request("/tools/rescan", { method: "POST" });
      if (expectedEpoch === epoch && expectedScope === control.scope() && control.available()) {
        notice = "Tool scan complete.";
        await load(expectedScope, expectedEpoch);
      }
    } catch (cause) {
      if (expectedEpoch === epoch && expectedScope === control.scope()) error = cause?.message || "Tool scan failed.";
    } finally { if (expectedEpoch === epoch) busy = false; }
  }

  onMount(() => {
    const update = (access) => {
      const nextScope = access?.scope || control.scope();
      if (nextScope !== scope || managedAvailable !== Boolean(access?.available)) {
        scope = nextScope;
        managedAvailable = Boolean(access?.available);
        epoch++;
        sequence++;
        managed = null;
        error = "";
      }
      if (managedAvailable) void load(scope, epoch);
    };
    const unsubscribe = control.subscribe(update);
    return () => { sequence++; unsubscribe?.(); };
  });

  const capabilities = $derived(managedAvailable ? managed?.tools : local?.capabilities);
  const tools = $derived(items(capabilities));
</script>

{#if error}<p class="setting-description companion-tool-error" role="alert">{error}</p>{/if}
{#if notice}<p class="setting-description" role="status">{notice}</p>{/if}
<SettingRow id="companion-rescan-tools" title="Detected tools" description={capabilities?.platform ? `Available on ${capabilities.platform}.` : "Local renderers and connected applications detected on this computer."}>
  {#if managedAvailable}<button class="btn btn-sm lp-control-outline" type="button" disabled={busy} onclick={() => void rescan()}>{busy ? "Scanning…" : "Rescan"}</button>{/if}
</SettingRow>
{#each tools as { name, tool } (name.toLowerCase())}
  <SettingRow title={name.charAt(0).toUpperCase() + name.slice(1)} description={tool?.note || (tool?.available ? "Ready to use from LibrePaper." : `Install ${name} on this computer, then rescan tools.`)}>
    <span class="setting-description">{tool?.available ? (tool.version ? `Available · ${tool.version}` : "Available") : "Not found"}</span>
  </SettingRow>
{:else}<p class="setting-description">{managedAvailable && !managed ? "Loading tool status…" : "Tool status is not available yet."}</p>{/each}

<style>
  .companion-tool-error { color: var(--color-error-text); }
</style>
