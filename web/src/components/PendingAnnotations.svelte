<script>
  let { items = [], onretry, ondiscard } = $props();
</script>

{#if items.length}
  <section class="card preset-outlined-warning-500 p-3 m-3" aria-label="Unconfirmed comments">
    <h3 class="h6">Unconfirmed comments</h3>
    <p class="text-sm">Your text is kept here until the server confirms it.</p>
    {#each items as item (item.message.temp_id)}
      <article class="mt-3">
        <p class="text-sm">{item.message.type === "reply" ? "Reply" : "Comment"}</p>
        <textarea class="textarea" rows="3" readonly aria-label="Unconfirmed text"
                  value={item.message.body || item.message.exact || "Figure annotation"}></textarea>
        <p class="text-sm" role="status">{item.error || "Waiting for confirmation…"}</p>
        <div class="flex gap-2 mt-2">
          <button type="button" class="btn btn-sm preset-tonal-primary"
                  onclick={() => onretry(item.message.temp_id)}>Retry</button>
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700"
                  onclick={() => ondiscard(item.message.temp_id)}>Discard draft</button>
        </div>
      </article>
    {/each}
  </section>
{/if}
