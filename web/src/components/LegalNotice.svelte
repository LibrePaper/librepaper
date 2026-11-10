<script>
  import { m } from "../lib/paraglide/messages.js";

  // The server's policy links, from /api/me. Missing, null and empty all mean "not set".
  /** @type {{legal?: {terms_url?: string | null, privacy_url?: string | null} | null}} */
  let { legal = undefined } = $props();
  const terms = $derived(legal?.terms_url || "");
  const privacy = $derived(legal?.privacy_url || "");
</script>

{#if terms || privacy}
  <span class="legal-notice">
    <span>{m.legalnotice_accept()}</span>
    {#if terms}<a href={terms} target="_blank" rel="noreferrer">{m.common_terms_of_service()}</a>{/if}
    {#if privacy}<a href={privacy} target="_blank" rel="noreferrer">{m.common_privacy_policy()}</a>{/if}
  </span>
{/if}

<style>
  .legal-notice {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.25rem 0.875rem;
    font-size: var(--text-sm);
    color: var(--color-text-secondary);
  }

  .legal-notice a {
    color: inherit;
    text-decoration: underline;
  }
</style>
