<script>
  // Wraps a feature that exists only in the desktop app. Inside the desktop app
  // it renders its children. In a browser it renders one notice with the ways
  // to get or open the desktop app.
  import { desktop, INSTALL_URL, DESKTOP_ONLY_MESSAGE } from "../lib/desktop/client.js";

  let { feature = "", id = undefined, children = undefined } = $props();

  const available = Boolean(desktop());
  const message = $derived(feature ? `${feature} needs the LibrePaper desktop app.` : DESKTOP_ONLY_MESSAGE);
  // Never the fragment: a share link's key would pass through the system's link handler and the app's command line. The app keeps its own storage, so a share link is pasted into the app instead (Projects, Open a link).
  const here = globalThis.location;
  const openLink = $derived(`librepaper://open?url=${encodeURIComponent(here ? here.origin + here.pathname + here.search : "")}`);
</script>

{#if available}
  {@render children?.()}
{:else}
  <div {id} class="desktop-only">
    <p class="desktop-only-message">{message}</p>
    <a class="btn btn-sm lp-control-tonal-brand" href={INSTALL_URL} target="_blank" rel="noreferrer">Get the desktop app</a>
    <a class="btn btn-sm lp-control-brand" href={openLink}>Open in the desktop app</a>
  </div>
{/if}

<style>
  .desktop-only {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: calc(var(--spacing) * 2);
    margin-block: calc(var(--spacing) * 3) calc(var(--spacing) * 2);
    font-size: var(--text-sm);
  }
  .desktop-only-message {
    flex-basis: 100%;
    margin: 0;
    color: var(--color-text-secondary);
  }
</style>
