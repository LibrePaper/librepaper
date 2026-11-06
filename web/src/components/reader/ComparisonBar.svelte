<script>
  import { m } from "../../lib/paraglide/messages.js";
  // The bar across the top of the editor area while a comparison is open. It
  // names what is being compared, and Close gives the editor area back.
  let { says, onclose } = $props();
</script>

<div class="comparison-bar" role="status">
  <p class="comparison-says">{says}</p>
  <button type="button" class="btn btn-sm" onclick={onclose}>{m.common_close()}</button>
</div>

<style>
  .comparison-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    padding: 0.5rem;
    border-bottom: 1px solid var(--color-border);
  }
  .comparison-says {
    flex: 1;
    min-width: 0;
    margin: 0;
    overflow: hidden;
    font-size: var(--text-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Phones: Close is a finger's height and keeps its place at the end. */
  @media (max-width: 767px) {
    .comparison-bar > .btn { flex: none; min-height: 2.75rem; }
  }
</style>
