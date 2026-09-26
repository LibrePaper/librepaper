<script>
  // The last thing between a thrown error and a blank page.
  //
  // Reader.svelte is three and a half thousand lines and every panel in the
  // application is inside it. Before this, one throw anywhere in that subtree
  // unmounted the lot and left the reader looking at an empty document with
  // nothing said and nothing logged.
  //
  // `<svelte:boundary>` renders no element of its own, which matters here:
  // the pages mount into <body> because the stylesheet addresses the bar as
  // `body > nav`, and a wrapper would quietly stop every one of those rules
  // from matching.
  import { report } from "../lib/crash.js";

  // `component` rather than a snippet, so an entry can keep saying
  // `mount(Boundary, { props: { component: Reader } })` and nothing else
  // changes about how a page is started.
  let { component: Component, props = {}, name = "the page" } = $props();
</script>

<svelte:boundary onerror={(error) => report(error, name)}>
  <Component {...props} />

  {#snippet failed(error, reset)}
    <!-- Deliberately plain. Whatever broke may be the toast store, the theme,
         or the icon set, so this leans on none of them. -->
    <div class="crash" role="alert">
      <h1>Something in LibrePaper stopped working</h1>
      <p>
        The rest of the page could not be drawn. Nothing you have written has
        been sent anywhere or lost — a document you were editing is still in
        this browser, and reloading will pick it up again.
      </p>
      <p class="crash-detail">{error?.message || "An unknown error"}</p>
      <div class="crash-actions">
        <!-- Trying again costs nothing and sometimes works: a chunk that
             failed to load is fetched again, and a bad bit of state is built
             from scratch. Reloading is the way out when it does not. -->
        <button type="button" onclick={reset}>Try again</button>
        <button type="button" onclick={() => location.reload()}>Reload the page</button>
      </div>
    </div>
  {/snippet}
</svelte:boundary>

<style>
  /* No colour and no token: the theme may be exactly what failed, and a notice
     that needs a stylesheet to be readable is no notice at all. Everything
     here inherits, so it renders against whatever the page already is. */
  .crash {
    margin: 3rem auto;
    max-width: 34rem;
    padding: 1.5rem;
    border: 1px solid;
    border-radius: 0.5rem;
    font-family: system-ui, sans-serif;
    line-height: 1.5;
  }
  .crash h1 { margin: 0 0 0.75rem; font-size: 1.25rem; }
  .crash p { margin: 0 0 0.75rem; }
  .crash-detail {
    font-family: ui-monospace, monospace;
    font-size: 0.8125rem;
    opacity: 0.7;
    overflow-wrap: anywhere;
  }
  .crash-actions { display: flex; gap: 0.5rem; }
  .crash-actions button {
    padding: 0.4rem 0.9rem;
    border: 1px solid currentcolor;
    border-radius: 0.375rem;
    background: none;
    color: inherit;
    cursor: pointer;
    font: inherit;
  }
</style>
