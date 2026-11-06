<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import Menubar from "./Menubar.svelte";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";
  import { signInHref, signOut } from "../lib/api.js";
  import { m } from "../lib/paraglide/messages.js";

  // The bar every page wears: whatever the page puts in the middle, and who
  // you are.
  //
  // One row, centred, with a gap: the vertical rhythm is decided here and
  // nowhere else, so a control added later cannot land half a line above its
  // neighbours.
  //
  // Page-specific tools may include compact global state, such as the
  // Reader's connection and presence indicator. Pane-specific state stays
  // with the pane it describes.
  //
  // `signIn` drops the Sign in link for the page that link leads to.
  //
  // `phoneShell` is the Reader's phone header: a back control and the title
  // take the room, the tools sit at the end, and the account and the menubar
  // give way (the account is in the More sheet, the menubar stays mounted for
  // its dialogs and shortcuts).
  let { me = {}, children = undefined, menus = undefined, tools = undefined, phoneMenu = false, phoneShell = false, hidden = false, signIn = true } = $props();
</script>

<!-- The way past the bar. Every page puts twenty-odd controls between the
     top of the document and the first thing on the page -- the menus, the
     title, presence, the account -- and this is one Tab and one Enter over
     all of them. It shows itself when it takes the focus and is
     invisible otherwise, which is the only time anyone needs it. -->
<a class="skip-link" href="#main">{m.nav_skip_to_content()}</a>

<nav class="flex items-center justify-between gap-4" class:phone-menu={phoneMenu} class:phone-shell={phoneShell} class:bar-hidden={hidden}>
  <div class="nav-identity flex min-w-0 items-center gap-3">
    {#if children}
      <span class="nav-trail flex min-w-0 items-center gap-3">
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
         is its handle and is shown to nobody, here least of all. A page with
         its own phone Menu moves the account into it, so this hides there.
         The provider it signed in with is a small logo badge on the corner of
         the circle, and the menu and the label say it in words. -->
    <span class="nav-account flex items-center">
      {#if me.name}
        {@const shown = me.provider === "github" ? `@${me.name}` : me.name}
        {@const provider = me.providers?.find((p) => p.id === me.provider)}
        {@const label = provider?.label || me.provider || ""}
        <Menu onSelect={(chosen) => { if (chosen.value === "signout") void signOut(); }}>
          <Menu.Trigger class="account icon-control" aria-label={label ? m.nav_account_menu_label_provider({ shown, provider: label }) : m.nav_account_menu_label({ shown })} title={label ? `${shown} · ${label}` : shown}>
            <span class="account-avatar">
              <Avatar name={me.name} key={me.handle || me.name} size={7} title="" />
              {#if provider}
                <span class="account-provider-badge" aria-hidden="true">
                  {#if provider.kind === "github"}
                    <Icon name="github" size={9} />
                  {:else if provider.kind === "google"}
                    <img src="/assets/google-g.svg" width="9" height="9" alt="" />
                  {:else if provider.kind === "orcid"}
                    <img src="/assets/orcid-id.svg" width="9" height="9" alt="" />
                  {:else}
                    <Icon name="key" size={9} />
                  {/if}
                </span>
              {/if}
            </span>
          </Menu.Trigger>
          <ExplorerMenu>
            <div class="account-who" aria-hidden="true">{shown}</div>
            {#if label}<div class="account-provider" aria-hidden="true">{m.nav_signed_in_with({ provider: label })}</div>{/if}
            <Menu.Item value="signout" class="menuitem">{m.common_sign_out()}</Menu.Item>
          </ExplorerMenu>
        </Menu>
      {:else if signIn && me.providers?.length}
        <a class="btn btn-sm lp-control-brand" href={signInHref()}>{m.common_sign_in()}</a>
      {/if}
    </span>
  </div>
</nav>

<style>
  .skip-link { position: absolute; left: calc(var(--spacing) * 2); top: calc(var(--spacing) * -12); z-index: 200; padding: calc(var(--spacing) * 2) calc(var(--spacing) * 3); border-radius: var(--radius-base); background: var(--color-brand); color: var(--color-on-brand); font-size: var(--text-sm); transition: top 0.15s; }
  /* `:focus`, not `:focus-visible`: this is only ever reached by moving the
     focus, and it has to show itself every time it is, however the focus got
     there. */
  .skip-link:focus { top: calc(var(--spacing) * 2); outline: 2px solid var(--color-on-brand); outline-offset: 2px; }
  /* The button is the circle: no box of its own, a ring on hover so it reads
     as something to press, and the focus outline every icon control wears. */
  :global(.account) { display: inline-grid; flex: none; place-items: center; padding: 0; border: 0; background: none; border-radius: 50%; cursor: pointer; line-height: 0; }
  :global(.account:hover), :global(.account[data-state="open"]) { box-shadow: 0 0 0 2px var(--color-border); }
  /* Who the menu belongs to: a label above the one action, not an action. */
  .account-who { padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2) calc(var(--spacing) * 0.5); font-size: var(--text-xs); color: var(--color-text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .account-provider { padding: 0 calc(var(--spacing) * 2) calc(var(--spacing) * 1.5); font-size: var(--text-xs); color: var(--color-text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* The provider badge: a 12px disc on the bottom-right of the circle, with
     the mark inside it at 9px. */
  .account-avatar { position: relative; display: inline-grid; line-height: 0; }
  .account-provider-badge { position: absolute; right: -3px; bottom: -3px; display: grid; place-items: center; width: calc(var(--spacing) * 3); height: calc(var(--spacing) * 3); border-radius: 50%; border: 1px solid var(--color-border); background: var(--color-raised); line-height: 0; }
  .account-provider-badge img, .account-provider-badge :global(svg) { width: 9px; height: 9px; }
  /* The identity group is clipped so that a menubar or page name too wide
     for the row cannot paint over the tools beside it. The rules below keep
     it from being reached. */
  .nav-identity { overflow: hidden; }
  .nav-trail { flex: 0 1 auto; }
  @media (max-width: 767px) {
    .nav-identity { flex: 0 1 auto; }
    .nav-actions { gap: var(--spacing); }
    .phone-menu .nav-account { display: none; }
  }
  /* Narrower than this the bar carries the tools and nothing else: the file
     name is squeezed to nothing here anyway, and both the Files panel and the
     mobile bar still name it. */
  @media (max-width: 600px) {
    nav { gap: var(--spacing); padding-inline: calc(var(--spacing) * 2); }
    /* Let the two groups share the narrow bar. Keeping both at their
       intrinsic width can make the document itself wider than the phone once
       a page adds its own view and status controls. */
    .nav-actions { gap: var(--spacing); flex-shrink: 1; min-width: 0; }
    .phone-menu .nav-actions { flex-shrink: 0; }
    .nav-identity { gap: var(--spacing); flex-shrink: 1; min-width: 0; }
    .nav-trail { display: none; }
  }
  @media (max-width: 767px) {
    .nav-trail { display: none; }
    /* The same 8px between the hamburger and the two faces as between the
       two faces themselves: one row of thumb-sized controls. */
    .phone-menu .nav-actions { gap: calc(var(--spacing) * 2); }
    /* At compact widths the menubar is kept mounted for keyboard navigation
       and the menu logic but rendered invisible and kept out of the flow;
       where its menus open is not decided here, a menu chosen from the Menu
       trigger hangs from that trigger (lib/menubar.svelte.js). */
    .phone-menu :global(.menubar) {
      position: absolute;
      top: 100%;
      left: calc(var(--spacing) * 2);
      height: 0;
      overflow: hidden;
      visibility: hidden;
      pointer-events: none;
    }
    /* The Reader's phone header: the trail (back, then the title) takes
       everything the tools leave and truncates, and the tools keep their
       width at the end. */
    nav.phone-shell {
      gap: 0;
      padding-top: env(safe-area-inset-top);
      padding-left: max(calc(var(--spacing) * 1), env(safe-area-inset-left));
      padding-right: max(calc(var(--spacing) * 1), env(safe-area-inset-right));
    }
    .phone-shell .nav-identity { flex: 1 1 auto; min-width: 0; gap: 0; }
    .phone-shell .nav-trail { display: flex; flex: 1 1 0; min-width: 0; gap: 0; }
    .phone-shell .nav-actions { flex: none; gap: 0; }
    .phone-shell .nav-account { display: none; }
  }
</style>
