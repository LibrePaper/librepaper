<script>
  import { onMount } from "svelte";
  import SettingRow from "./SettingRow.svelte";
  import { loadStorageStatus, storageBytes } from "../../lib/quota-preferences.js";

  let snapshot = $state(null);
  let loading = $state(true);
  let error = $state("");
  let alive = true;
  let generation = 0;

  const usage = $derived(snapshot?.usage);
  const percent = $derived(
    usage?.hardQuotaBytes > 0 && Number.isFinite(usage.chargedBytes)
      ? Math.round((100 * usage.chargedBytes) / usage.hardQuotaBytes)
      : null,
  );
  const policy = $derived(snapshot?.policy || {});

  async function reload() {
    const job = ++generation;
    loading = true;
    error = "";
    try {
      const result = await loadStorageStatus();
      if (alive && job === generation) snapshot = result;
    } catch (cause) {
      if (alive && job === generation) error = cause.message || "Storage status could not be loaded.";
    } finally {
      if (alive && job === generation) loading = false;
    }
  }

  onMount(() => {
    void reload();
    return () => {
      alive = false;
      generation += 1;
    };
  });
</script>

<div class="flex flex-col gap-6">
  {#if error}<p role="alert">{error}</p>{/if}
  {#if loading}<p>Loading storage status…</p>
  {:else if snapshot}
    <section aria-label="Storage usage" class="flex flex-col gap-2">
      <h3 class="h5">Storage</h3>
      <p>{storageBytes(usage?.chargedBytes)} of {storageBytes(usage?.hardQuotaBytes)}{percent == null ? "" : ` (${percent}%)`}</p>
      <button class="btn btn-sm preset-outlined-surface-300-700 self-start" type="button" onclick={reload}>Refresh storage status</button>
    </section>

    <SettingRow id="quota-policy" title="Retention policy" description="Automatic checkpoint cleanup is fixed across this deployment.">
      <p>Keep up to {policy.routineVersionCount} unlabeled checkpoints and keep only checkpoints newer than {policy.routineVersionAgeDays} days.</p>
      <p>Named checkpoints remain protected indefinitely.</p>
      <p>Checkpoints are pruned during maintenance; a checkpoint may remain briefly until maintenance catches up.</p>
      <p>Maximum retained checkpoints per account: {policy.maxCheckpointCount || "undefined"}.</p>
    </SettingRow>
  {/if}
</div>
