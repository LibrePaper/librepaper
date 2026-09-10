<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Logo from "./Logo.svelte";
  import IconButton from "./IconButton.svelte";
  import Icon from "./Icon.svelte";
  import { signInHref, signOut } from "../lib/api.js";

  // The bar every page wears: the logo, whatever the page puts in the middle,
  // and who you are.
  //
  // One row, centred, with a gap: the vertical rhythm is decided here and
  // nowhere else, so a control added later cannot land half a line above its
  // neighbours.
  let { me = {}, children, menus, tools, status, documentation = true } = $props();
</script>

<nav class="flex items-center justify-between gap-4">
  <div class="nav-identity flex min-w-0 items-center gap-3">
    <a class="flex items-center gap-2" href="/" aria-label="LibrePaper home">
      <Logo />
    </a>
    <!-- The separator and the name it separates are one thing, so a bar too
         narrow to show the name drops the slash with it rather than leaving
         it standing on its own. -->
    {#if children}
      <span class="nav-trail flex min-w-0 items-center gap-3">
        <span class="nav-divider" aria-hidden="true">/</span>
        {@render children()}
      </span>
    {/if}
    {#if menus}<div class="flex shrink-0 items-center gap-2">{@render menus()}</div>{/if}
  </div>

  {#if status}<div class="nav-status" role="status">{@render status()}</div>{/if}

  <div class="nav-actions flex shrink-0 items-center gap-2">
    {@render tools?.()}
    <!-- The one link that is the same on every page: what LibrePaper is and how
         to use it, from the project's own README. An icon among the other
         icons rather than a phrase in the middle of the bar, which is width
         the document title wanted and a shape nothing else in the bar had. -->
    {#if documentation}
      <IconButton icon="help" label="Documentation" href="/documentation" />
    {/if}
    <!-- A GitHub account is its login, and the @ is what says so. A Google
         account is a profile name, which is not a handle and does not wear
         one; its email is its handle and is shown to nobody, here least of
         all. -->
    {#if me.name}
      <Menu onSelect={(chosen) => { if (chosen.value === "signout") void signOut(); }}>
        <Menu.Trigger class="btn btn-sm preset-outlined-surface-300-700" aria-label="Account menu">
          <span class="account-name">{me.provider === "github" ? `@${me.name}` : me.name}</span>
          <span class="account-compact" aria-hidden="true"><Icon name="users" /></span><span aria-hidden="true">▾</span>
        </Menu.Trigger>
        <ExplorerMenu>
          <Menu.Item value="signout" class="menuitem">Sign out</Menu.Item>
        </ExplorerMenu>
      </Menu>
    {:else if me.providers?.length}
      <a role="button" class="btn btn-sm preset-filled-primary-500" href={signInHref()}>Sign in</a>
    {/if}
  </div>
</nav>

<style>
  .account-compact { display: none; }
  .nav-divider { color: var(--color-surface-400-600); user-select: none; flex: none; }
  /* The logo is not allowed to shrink, so without this it paints over
     whatever the bar puts beside it as soon as the row runs out of room.
     Clipping is the floor; the rules below are what keep it from being
     reached. */
  .nav-identity { overflow: hidden; }
  .nav-trail { flex: 0 1 auto; }
  @media (max-width: 760px) {
    .nav-identity { flex: 0 1 auto; }
    .nav-actions { gap: var(--spacing); }
    .account-name { display: none; }
    .account-compact { display: inline; }
  }
  /* Narrower than this the bar carries the logo, the status and the tools and
     nothing else: the file name is squeezed to nothing here anyway, and both
     the Files panel and the mobile bar still name it. */
  @media (max-width: 600px) {
    nav { gap: var(--spacing); padding-inline: calc(var(--spacing) * 2); }
    .nav-identity { gap: var(--spacing); flex-shrink: 0; }
    .nav-trail { display: none; }
  }
</style>
