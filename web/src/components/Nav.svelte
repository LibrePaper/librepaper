<script>
  import Logo from "./Logo.svelte";
  import IconButton from "./IconButton.svelte";
  import Row from "./layout/Row.svelte";
  import { signInHref, signOut } from "../lib/api.js";

  // The bar every page wears: the mark, whatever the page puts in the middle,
  // and who you are.
  //
  // One row, centred, with a gap: the vertical rhythm is decided here and
  // nowhere else, so a control added later cannot land half a line above its
  // neighbours.
  let { me = {}, children, tools, status } = $props();
</script>

<nav class="flex items-center justify-between gap-4">
  <div class="nav-identity flex min-w-0 items-center gap-3">
    <a class="flex items-center gap-2" href="/" aria-label="Komodoc home">
      <Logo />
      <strong class="wordmark"><span class="wordmark-komo">komo</span><span class="wordmark-doc">doc</span></strong>
    </a>
    {@render children?.()}
  </div>

  {#if status}<div class="nav-status" role="status">{@render status()}</div>{/if}

  <div class="nav-actions flex shrink-0 items-center gap-3">
    {@render tools?.()}
    <!-- The one link that is the same on every page: what Komodoc is and how
         to use it, from the project's own README. An icon among the other
         icons rather than a phrase in the middle of the bar, which is width
         the document title wanted and a shape nothing else in the bar had. -->
    <IconButton icon="help" label="Documentation" href="/documentation" />
    <!-- A GitHub account is its login, and the @ is what says so. A Google
         account is a profile name, which is not a handle and does not wear
         one; its email is its handle and is shown to nobody, here least of
         all. -->
    {#if me.name}
      <small class="text-surface-600-400 whitespace-nowrap"
        >{me.provider === "github" ? `@${me.name}` : me.name}</small
      >
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={signOut}>
        Sign out
      </button>
    {:else if me.providers?.length}
      <a role="button" class="btn btn-sm preset-filled-primary-500" href={signInHref()}>Sign in</a>
    {/if}
  </div>
</nav>
