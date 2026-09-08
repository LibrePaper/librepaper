<script>
  let { items = [], onretry, ondiscard } = $props();
</script>

{#if items.length}
  <section class="text-sm" aria-label="Unconfirmed comments">
    {#each items as item (item.message.temp_id)}
      <details class="border-surface-200-800 rounded border p-2">
        <summary class="cursor-pointer">
          <span>{item.message.type === "reply" ? "Reply" : "Comment"} not sent</span>
          <span class="text-surface-600-400 block truncate">{item.message.body || item.message.exact || "Figure annotation"}</span>
        </summary>
        <p class="text-surface-600-400 mt-2">Draft saved on this device.</p>
        <textarea class="textarea mt-2" rows="2" readonly aria-label="Unconfirmed text"
                  value={item.message.body || item.message.exact || "Figure annotation"}></textarea>
        <p class="text-sm" role="status">{item.error || "Waiting for confirmation…"}</p>
        <div class="flex gap-2 mt-2">
          <button type="button" class="btn btn-sm preset-tonal-primary"
                  onclick={() => onretry(item.message.temp_id)}>Retry</button>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                  onclick={() => ondiscard(item.message.temp_id)}>Discard draft</button>
        </div>
      </details>
    {/each}
  </section>
{/if}
