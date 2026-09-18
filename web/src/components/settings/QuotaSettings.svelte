<script>
  // What this account is storing on this deployment, and what a version is.
  // The account's, not the document's: it says the same thing whatever
  // happens to be open.
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
  // One line under the title, whatever the status of the request: the row
  // keeps its shape while the numbers are on their way or never arrive.
  const used = $derived(
    error ? error
    : loading ? "Measuring…"
    : `${storageBytes(usage?.chargedBytes)} of ${storageBytes(usage?.hardQuotaBytes)}${percent == null ? "" : ` (${percent}%)`} used.`,
  );

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

<SettingRow id="storage-account" title="Account storage" description={used}>
  <button class="btn btn-sm preset-outlined-surface-300-700" type="button" onclick={reload}>Refresh</button>
</SettingRow>

{#if snapshot}
  <SettingRow id="storage-retention" stacked title="Versions"
              description="Nothing is deleted on a schedule.">
    <p>A version is written when you name one, publish, restore an earlier
      version, or leave a comment — never on a timer.</p>
    <p>Every version is kept until you delete the document.</p>
    <p>Editing between versions is not lost: the full editing history is kept
      separately, and the history panel can show the document as it stood at
      any moment in it, whether or not anybody named that moment.</p>
  </SettingRow>
{/if}
