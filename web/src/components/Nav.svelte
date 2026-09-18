<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Menubar from "./Menubar.svelte";
  import Logo from "./Logo.svelte";
  import Avatar from "./Avatar.svelte";
  import { signInHref, signOut } from "../lib/api.js";

  // The bar every page wears: the logo, whatever the page puts in the middle,
  // and who you are.
  //
  // One row, centred, with a gap: the vertical rhythm is decided here and
  // nowhere else, so a control added later cannot land half a line above its
  // neighbours.
  //
  // Page-specific tools may include compact global state, such as the
  // Reader's connection and presence indicator. Pane-specific state stays
  // with the pane it describes.
  let { me = {}, children, menus, tools } = $props();
</script>

<!-- The way past the bar. Every page puts twenty-odd controls between the
     top of the document and the first thing on the page -- the logo, the
     menus, the title, presence, the account -- and this is one Tab and one
     Enter over all of them. It shows itself when it takes the focus and is
     invisible otherwise, which is the only time anyone needs it. -->
<a class="skip-link" href="#main">Skip to content</a>

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
    {#if menus}<Menubar>{@render menus()}</Menubar>{/if}
  </div>

  <div class="nav-actions flex shrink-0 items-center gap-2">
    {@render tools?.()}
    <!-- The account's initials, on the colour that name always wears, and
         nothing else: the name itself waits in the menu behind it. A GitHub
         account is its login, and the @ is what says so. A Google account is a
         profile name, which is not a handle and does not wear one; its email
         is its handle and is shown to nobody, here least of all. -->
    {#if me.name}
      {@const shown = me.provider === "github" ? `@${me.name}` : me.name}
      <Menu onSelect={(chosen) => { if (chosen.value === "signout") void signOut(me.site); }}>
        <Menu.Trigger class="account icon-control" aria-label={`Account menu, signed in as ${shown}`} title={shown}>
          <Avatar name={me.name} key={me.handle || me.name} size={7} title="" />
        </Menu.Trigger>
        <ExplorerMenu>
          <div class="account-who" aria-hidden="true">{shown}</div>
          <Menu.Item value="signout" class="menuitem">Sign out</Menu.Item>
        </ExplorerMenu>
      </Menu>
    {:else if me.providers?.length}
      <a class="btn btn-sm preset-filled-primary-500" href={signInHref()}>Sign in</a>
    {/if}
  </div>
</nav>

<style>
  .skip-link { position: absolute; left: calc(var(--spacing) * 2); top: calc(var(--spacing) * -12); z-index: 200; padding: calc(var(--spacing) * 2) calc(var(--spacing) * 3); border-radius: var(--radius-base); background: var(--color-primary-500); color: white; font-size: var(--text-sm); transition: top 0.15s; }
  /* `:focus`, not `:focus-visible`: this is only ever reached by moving the
     focus, and it has to show itself every time it is, however the focus got
     there. */
  .skip-link:focus { top: calc(var(--spacing) * 2); outline: 2px solid var(--color-surface-950-50); outline-offset: 2px; }
  /* The button is the circle: no box of its own, a ring on hover so it reads
     as something to press, and the focus outline every icon control wears. */
  :global(.account) { display: inline-grid; place-items: center; padding: 0; border: 0; background: none; border-radius: 50%; cursor: pointer; line-height: 0; }
  :global(.account:hover), :global(.account[data-state="open"]) { box-shadow: 0 0 0 2px var(--color-surface-300-700); }
  /* Who the menu belongs to: a label above the one action, not an action. */
  .account-who { padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2) calc(var(--spacing) * 0.5); font-size: var(--text-xs); color: var(--color-surface-600-400); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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
  }
  /* Narrower than this the bar carries the logo and the tools and nothing
     else: the file name is squeezed to nothing here anyway, and both the
     Files panel and the mobile bar still name it. */
  @media (max-width: 600px) {
    nav { gap: var(--spacing); padding-inline: calc(var(--spacing) * 2); }
    .nav-identity { gap: var(--spacing); flex-shrink: 0; }
    .nav-trail { display: none; }
  }
</style>
