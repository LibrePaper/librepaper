<script>
  // Comments and replies the room has not confirmed. Each is one quiet line
  // under the panel; opening it shows the draft and offers a retry.
  let { items = [], onretry, ondiscard } = $props();
</script>

{#if items.length}
  <section class="pending text-xs" aria-label="Unconfirmed comments">
    {#each items as item (item.message.temp_id)}
      <details>
        <summary class="cursor-pointer">
          <span class="dot" aria-hidden="true"></span>
          <span class="kind">{item.message.type === "reply" ? "Reply" : "Comment"} not sent</span>
          <span class="text-surface-600-400 truncate">{item.message.body || item.message.exact || "Figure annotation"}</span>
        </summary>
        <div class="body">
          <textarea class="textarea" rows="2" readonly aria-label="Unconfirmed text"
                    value={item.message.body || item.message.exact || "Figure annotation"}></textarea>
          <p class="text-surface-600-400" role="status">{item.error || "Waiting for confirmation…"} Draft saved on this device.</p>
          <div class="flex gap-2">
            <button type="button" class="btn btn-sm preset-tonal-primary"
                    onclick={() => onretry(item.message.temp_id)}>Retry</button>
            <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                    onclick={() => ondiscard(item.message.temp_id)}>Discard draft</button>
          </div>
        </div>
      </details>
    {/each}
  </section>
{/if}

<style>
  details + details { border-top: 1px solid var(--color-surface-200-800); }
  summary { display: flex; align-items: center; gap: .4rem; min-width: 0; padding: .35rem .75rem; list-style: none; }
  summary::-webkit-details-marker { display: none; }
  .dot { flex-shrink: 0; width: .45rem; height: .45rem; border-radius: 50%; background: var(--color-warning-500); }
  .kind { flex-shrink: 0; font-weight: 600; }
  .truncate { flex: 1 1 auto; min-width: 0; }
  .body { display: flex; flex-direction: column; gap: .5rem; padding: 0 .75rem .6rem; }
</style>
